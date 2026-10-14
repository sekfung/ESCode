//! 双传输 Link（docs/specs/rust-u1-runtime-lib.md §3.3）：stdio 与内存双工链路。
//!
//! 行切分、帧上限、dispatch、EOF/关闭语义与输出停滞看门狗全部在 [`crate::stdio::start_io`]
//! 一条代码路径上——等价性由「同一段代码」保证。`MemoryLink` 先以 mpsc 字节通道 +
//! NDJSON 保帧面等价，接口形状为阶段 4 免序列化直传预留。

use std::io::{Read, Write};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::contract::{Input, Output};
use crate::stdio::start_io;

/// 一次打开的协议流：引擎输入、输出端与写出线程收口句柄。
pub struct Streams {
    pub input: mpsc::Receiver<Input>,
    pub output: Output,
    pub writer: std::thread::JoinHandle<()>,
}

/// 传输抽象：打开协议流。链路之上代码不区分传输。
/// Link 是一次性资源（打开即消费两端），stdio 与内存链路同形。
pub trait Link: Send + Sync + 'static {
    fn start(
        self: Box<Self>,
        cancel: CancellationToken,
        input_closed: CancellationToken,
    ) -> Streams;
}

/// 进程 stdin/stdout 传输（既有 `app-server --stdio` 行为，逐字节不变）。
pub struct StdioLink;

impl Link for StdioLink {
    fn start(
        self: Box<Self>,
        cancel: CancellationToken,
        input_closed: CancellationToken,
    ) -> Streams {
        // 锁不是 Send：在线程内获取（工厂闭包）。StdinLock/StdoutLock 自 Rust 1.61 起为 'static。
        start_io(
            || std::io::stdin().lock(),
            || std::io::stdout().lock(),
            cancel,
            input_closed,
        )
    }
}

/// 嵌入调用方一侧的链路端点：发送入站 NDJSON 行、按行接收出站 NDJSON。
/// drop 即关闭入站（实例按 EOF 语义收口）；`line()` 返回 None = 出站已排空关闭。
pub struct EmbeddedIo {
    inbound: mpsc::Sender<Vec<u8>>,
    outbound: mpsc::Receiver<Vec<u8>>,
    /// 出站按行缓冲：writer 每帧分两次写出（JSON 字节 + 换行），line() 必须给完整行。
    pending: Vec<u8>,
}

/// 嵌入链路按读写拆开（Host 的帧泵长期持有读端，写端必须能并发进请求）。
pub struct EmbeddedIoReader {
    outbound: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

pub struct EmbeddedIoWriter {
    inbound: mpsc::Sender<Vec<u8>>,
}

impl EmbeddedIoReader {
    /// 接收一行出站 NDJSON（含结尾换行）；None = 实例 output 已关闭。
    pub async fn line(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let rest = self.pending.split_off(end + 1);
                return Some(std::mem::replace(&mut self.pending, rest));
            }
            match self.outbound.recv().await {
                Some(chunk) => self.pending.extend(chunk),
                None => return None,
            }
        }
    }
}

impl EmbeddedIoWriter {
    /// 发送一行 NDJSON（含或不含结尾换行均可）；实例已收口时丢弃（与写 stdin 到已退出进程一致）。
    pub async fn send_line(&self, line: impl AsRef<[u8]>) {
        let mut bytes = line.as_ref().to_vec();
        if bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        let _ = self.inbound.send(bytes).await;
    }
}

impl EmbeddedIo {
    /// 拆成读写两端：读端可被单个泵独占，写端可并发使用。
    pub fn split(self) -> (EmbeddedIoReader, EmbeddedIoWriter) {
        (
            EmbeddedIoReader {
                outbound: self.outbound,
                pending: self.pending,
            },
            EmbeddedIoWriter {
                inbound: self.inbound,
            },
        )
    }

    /// 发送一行 NDJSON（含或不含结尾换行均可；帧上限与 stdio 同一检查）。
    pub async fn send_line(&self, line: impl AsRef<[u8]>) {
        let mut bytes = line.as_ref().to_vec();
        if bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        // 实例已收口时入站丢弃：语义与写 stdin 到已退出进程一致。
        let _ = self.inbound.send(bytes).await;
    }

    /// 接收一行出站 NDJSON（含结尾换行）；None = 实例 output 已关闭。
    pub async fn line(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let rest = self.pending.split_off(end + 1);
                return Some(std::mem::replace(&mut self.pending, rest));
            }
            match self.outbound.recv().await {
                Some(chunk) => self.pending.extend(chunk),
                // 收口时残尾不成行（写入半途失败）则丢弃，与 stdout 截断一致。
                None => return None,
            }
        }
    }
}

/// 嵌入实例一侧的内存链路：与调用方在同一进程内经有界字节通道互传 NDJSON。
pub struct MemoryLink {
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Vec<u8>>,
}

impl MemoryLink {
    /// 建立链路两端；`capacity` 是每个方向的行数上界（背压与 stdio 管道一致）。
    pub fn channel(capacity: usize) -> (Self, EmbeddedIo) {
        let (inbound_tx, inbound_rx) = mpsc::channel(capacity);
        let (outbound_tx, outbound_rx) = mpsc::channel(capacity);
        (
            Self {
                inbound: inbound_rx,
                outbound: outbound_tx,
            },
            EmbeddedIo {
                inbound: inbound_tx,
                outbound: outbound_rx,
                pending: Vec::new(),
            },
        )
    }
}

impl Link for MemoryLink {
    fn start(
        self: Box<Self>,
        cancel: CancellationToken,
        input_closed: CancellationToken,
    ) -> Streams {
        let Self { inbound, outbound } = *self;
        start_io(
            move || {
                std::io::BufReader::new(ChannelRead {
                    rx: inbound,
                    buf: Vec::new(),
                    pos: 0,
                })
            },
            move || ChannelWrite { tx: outbound },
            cancel,
            input_closed,
        )
    }
}

/// `Read` over mpsc 字节通道：在阻塞读取线程上用 `blocking_recv`（该线程不在 tokio
/// runtime 内，阻塞安全）；sender 全部 drop = 0 字节 = EOF，与 stdin EOF 语义一致。
struct ChannelRead {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelRead {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.buf.len() {
            match self.rx.blocking_recv() {
                Some(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                None => return Ok(0),
            }
        }
        let take = (self.buf.len() - self.pos).min(out.len());
        out[..take].copy_from_slice(&self.buf[self.pos..self.pos + take]);
        self.pos += take;
        Ok(take)
    }
}

/// `Write` over mpsc 字节通道：有界通道写满即阻塞（背压与 stdio 管道一致），
/// 接收端关闭视作断管（触发既有「写出失败 → cancel」收口路径）。
struct ChannelWrite {
    tx: mpsc::Sender<Vec<u8>>,
}

impl Write for ChannelWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tx
            .blocking_send(buf.to_vec())
            .map(|()| buf.len())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "link closed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
