//! 双传输一致性（docs/specs/rust-u1-runtime-lib.md §3.3/§4）：同一场景经 MemoryLink
//! 与 StdioLink（进程内 std::io::pipe 扮演 stdin/stdout）双跑，断言帧序列、顺序与关闭语义等价。
//!
//! 两侧共用 `stdio::start_io` 的同一段读写代码；本测试守护「同一段代码 = 相同行为」这一
//! 前提，并在阶段 4 换成类型化直传后继续守住规格同源。
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use escode_cli_app_server::Link;
use escode_cli_app_server::stdio::Input;
use escode_cli_rust::app::Engine;
use escode_cli_rust::contract::{
    ContextPort, Output, RuntimeClock, RuntimePorts, SessionStore, ToolPort,
};
use escode_cli_rust::domain::prompt::{InstructionSource, PromptSnapshot};
use escode_cli_rust::domain::session::Session;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// 输入序列的比较投影：帧序号 + 变体 + 载荷（Input 未实现 PartialEq 的 Debug 面）。
fn summary(input: &Input) -> String {
    match input {
        Input::Request(request) => format!(
            "request method={} id={:?} params={}",
            request.method, request.id, request.params
        ),
        Input::Response {
            id,
            result,
            error,
            raw_result,
        } => format!("response id={id} result={result} error={error:?} raw={raw_result:?}"),
        Input::Invalid => "invalid".to_owned(),
        Input::TooLarge => "too-large".to_owned(),
        Input::Eof => "eof".to_owned(),
    }
}

/// 从写出线程读回的字节 → NDJSON 行。
fn read_lines(bytes: Vec<u8>) -> Vec<String> {
    String::from_utf8(bytes)
        .expect("protocol output is UTF-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

/// 场景：一串协议行（含无效 JSON、空白行、无换行尾行），断言两侧产出同一批 Input。
#[tokio::test]
async fn both_links_deliver_the_same_input_sequence() {
    let lines: Vec<&str> = vec![
        r#"{"method":"capabilities","id":"a","params":{}}"#,
        "",
        "   ",
        "{not json",
        r#"{"id":"r1","result":{"ok":true}}"#,
        r#"{"method":"session/create","id":"b","params":{"sessionId":"s1"}}"#,
    ];
    let payload = format!("{}\n", lines.join("\n"));

    // MemoryLink：载荷作为一块字节送入（块内含换行，等价于管道分块边界）。
    let (link, io) = escode_cli_app_server::MemoryLink::channel(8);
    let streams = Box::new(link).start(CancellationToken::new(), CancellationToken::new());
    let memory_input = collect_inputs(streams.input);
    io.send_line(&payload).await;
    drop(io);
    let memory = memory_input.await;

    // StdioLink 的同一段读写代码：进程内以 pipe 扮演 stdin/stdout。
    let (stdin_reader, mut stdin_writer) = std::io::pipe().expect("pipe");
    std::thread::spawn(move || {
        let _ = stdin_writer.write_all(payload.as_bytes());
        let _ = stdin_writer.flush();
        // 线程结束即关闭写端 → 读端 EOF。
    });
    let (_stdout_reader, stdout_writer) = std::io::pipe().expect("pipe");
    let streams = escode_cli_app_server::stdio::start_io(
        move || std::io::BufReader::new(stdin_reader),
        move || stdout_writer,
        CancellationToken::new(),
        CancellationToken::new(),
    );
    let stdio_input = collect_inputs(streams.input);
    let stdio = stdio_input.await;

    assert_eq!(
        memory, stdio,
        "same byte stream must produce the same Input sequence on both links"
    );
    assert_eq!(memory.last().map(String::as_str), Some("eof"));
}

/// 同场景下两侧输出端的帧字节等价：写同样批次，读回同样 NDJSON。
#[tokio::test]
async fn both_links_emit_the_same_frame_bytes() {
    use serde_json::json;
    let batch = vec![
        json!({"method":"session/stream","params":{"delta":"hello"}}),
        json!({"method":"session/stream","params":{"delta":"世界"}}),
    ];

    let (link, mut io) = escode_cli_app_server::MemoryLink::channel(8);
    let streams = Box::new(link).start(CancellationToken::new(), CancellationToken::new());
    streams
        .output
        .send(batch.clone())
        .await
        .expect("engine output open");
    drop(streams.output);
    let mut memory_bytes = Vec::new();
    while let Some(chunk) = io.line().await {
        memory_bytes.extend(chunk);
    }
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("memory writer drains");
    let memory_lines = read_lines(memory_bytes);

    let (stdout_reader, stdout_writer) = std::io::pipe().expect("pipe");
    let streams = escode_cli_app_server::stdio::start_io(
        || std::io::BufReader::new(std::io::empty()),
        move || stdout_writer,
        CancellationToken::new(),
        CancellationToken::new(),
    );
    streams
        .output
        .send(batch)
        .await
        .expect("engine output open");
    drop(streams.output);
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("stdio writer drains");
    let mut bytes = Vec::new();
    let mut stdout_reader = stdout_reader;
    stdout_reader.read_to_end(&mut bytes).expect("stdout read");
    let stdio_lines = read_lines(bytes);

    assert_eq!(
        memory_lines, stdio_lines,
        "both links must serialize identical NDJSON frames"
    );
    assert_eq!(memory_lines.len(), 2);
}

/// 关闭语义等价：输入侧 EOF 之后两侧都取消 `input_closed`；输出侧 drop output 都让 writer 收口结束。
#[tokio::test]
async fn both_links_share_eof_and_close_semantics() {
    let (link, io) = escode_cli_app_server::MemoryLink::channel(8);
    let closed = CancellationToken::new();
    let streams = Box::new(link).start(CancellationToken::new(), closed.clone());
    drop(io);
    drop(streams.output);
    tokio::time::timeout(std::time::Duration::from_secs(5), closed.cancelled())
        .await
        .expect("memory link reports EOF");
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("memory writer drains on output drop");

    let (stdin_reader, stdin_writer) = std::io::pipe().expect("pipe");
    let closed = CancellationToken::new();
    let streams = escode_cli_app_server::stdio::start_io(
        move || std::io::BufReader::new(stdin_reader),
        std::io::sink,
        CancellationToken::new(),
        closed.clone(),
    );
    drop(stdin_writer);
    drop(streams.output);
    tokio::time::timeout(std::time::Duration::from_secs(5), closed.cancelled())
        .await
        .expect("stdio link reports EOF");
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("stdio writer drains on output drop");
}

async fn collect_inputs(mut input: tokio::sync::mpsc::Receiver<Input>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Some(value) = input.recv().await {
        // TooLarge 与 Eof 都是读取端的终态：超限行之后不再有输入。
        let last = matches!(value, Input::Eof | Input::TooLarge);
        seen.push(summary(&value));
        if last {
            break;
        }
    }
    seen
}

/// 一条超过 `MAX_REQUEST_BYTES`（1 MiB）的 NDJSON 行。
fn oversize_payload() -> Vec<u8> {
    let mut payload = vec![b'x'; escode_cli_domain::MAX_REQUEST_BYTES + 1];
    payload.push(b'\n');
    payload
}

/// `MAX_REQUEST_BYTES` 超限：两种 Link 只交出 `too-large` 终态，不派发被截断的行。
#[tokio::test]
async fn both_links_flag_requests_over_max_bytes() {
    let payload = oversize_payload();

    let (link, io) = escode_cli_app_server::MemoryLink::channel(8);
    let streams = Box::new(link).start(CancellationToken::new(), CancellationToken::new());
    let memory_input = collect_inputs(streams.input);
    io.send_line(&payload).await;
    drop(io);
    let memory = memory_input.await;

    let (stdin_reader, mut stdin_writer) = std::io::pipe().expect("pipe");
    let writer = std::thread::spawn(move || {
        let _ = stdin_writer.write_all(&payload);
    });
    let streams = escode_cli_app_server::stdio::start_io(
        move || std::io::BufReader::new(stdin_reader),
        std::io::sink,
        CancellationToken::new(),
        CancellationToken::new(),
    );
    let stdio = collect_inputs(streams.input).await;
    writer.join().expect("stdin writer");

    assert_eq!(memory, vec!["too-large".to_owned()]);
    assert_eq!(
        memory, stdio,
        "oversize request must classify identically on both links"
    );
}

/// 同一超限输入经真实 Engine：两条 Link 的错误帧逐字节相等。
#[tokio::test(flavor = "multi_thread")]
async fn both_links_emit_the_same_oversize_error_frame() {
    let payload = oversize_payload();

    let (link, mut io) = escode_cli_app_server::MemoryLink::channel(8);
    let streams = Box::new(link).start(CancellationToken::new(), CancellationToken::new());
    let serving = start_engine(streams.input, streams.output).await;
    io.send_line(&payload).await;
    let mut memory_bytes = Vec::new();
    while let Some(line) = io.line().await {
        memory_bytes.extend(line);
    }
    serving
        .await
        .expect("engine task joins")
        .expect("engine exits cleanly");
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("memory writer drains");

    let (stdin_reader, mut stdin_writer) = std::io::pipe().expect("pipe");
    let writer = std::thread::spawn(move || {
        let _ = stdin_writer.write_all(&payload);
    });
    let (mut stdout_reader, stdout_writer) = std::io::pipe().expect("pipe");
    let streams = escode_cli_app_server::stdio::start_io(
        move || std::io::BufReader::new(stdin_reader),
        move || stdout_writer,
        CancellationToken::new(),
        CancellationToken::new(),
    );
    let serving = start_engine(streams.input, streams.output).await;
    // stdout 读取放到阻塞池：engine 与写出线程需要 runtime 继续推进。
    let reading = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        stdout_reader.read_to_end(&mut bytes).expect("stdout read");
        bytes
    });
    let stdio_bytes = reading.await.expect("stdout reader joins");
    writer.join().expect("stdin writer");
    serving
        .await
        .expect("engine task joins")
        .expect("engine exits cleanly");
    escode_cli_app_server::finish(streams.writer)
        .await
        .expect("stdio writer drains");

    assert_eq!(
        memory_bytes, stdio_bytes,
        "both links must emit byte-identical oversize error frames"
    );
    let lines = read_lines(memory_bytes);
    assert_eq!(lines.len(), 1, "one error frame expected: {lines:?}");
    let frame: Value = serde_json::from_str(&lines[0]).expect("error frame parses");
    assert_eq!(frame["error"]["code"], -32600);
    assert_eq!(frame["error"]["message"], "Request exceeds size limit");
}

/// 只走 `Input::TooLarge` 分支的最小 Engine；端口在超限路径不会被调用。
async fn start_engine(
    input: tokio::sync::mpsc::Receiver<Input>,
    output: Output,
) -> tokio::task::JoinHandle<Result<()>> {
    let engine = Engine::new(
        "oversize-fixture".to_owned(),
        None,
        RuntimePorts {
            context: Arc::new(UnusedContext),
            store: Arc::new(UnusedStore),
            model: None,
            tools: Arc::new(UnusedTools),
            clock: Arc::new(FixedClock),
            spawner: escode_cli_rust::contract::Spawner::current(),
        },
    )
    .await
    .expect("engine builds");
    tokio::spawn(engine.serve(input, output, CancellationToken::new()))
}

struct UnusedStore;

#[async_trait]
impl SessionStore for UnusedStore {
    async fn load_index(&self, _workspace: &str) -> Result<BTreeMap<String, Value>> {
        Ok(BTreeMap::new())
    }
    async fn lookup_ack(&self, _workspace: &str, _key: &str) -> Result<Option<Value>> {
        Ok(None)
    }
    async fn load(&self, _workspace: &str) -> Result<(Vec<Session>, BTreeMap<String, Value>)> {
        Ok((Vec::new(), BTreeMap::new()))
    }
    async fn commit(
        &self,
        _workspace: &str,
        _session: Option<&mut Session>,
        _ack: Option<(String, Value)>,
    ) -> Result<()> {
        Ok(())
    }
}

struct UnusedContext;

#[async_trait]
impl ContextPort for UnusedContext {
    fn desktop(&self) -> bool {
        false
    }
    async fn snapshot(&self, _cancel: &CancellationToken) -> Result<PromptSnapshot> {
        Ok(PromptSnapshot::default())
    }
    async fn instructions(&self, _cancel: &CancellationToken) -> Result<Vec<InstructionSource>> {
        Ok(Vec::new())
    }
}

struct UnusedTools;

#[async_trait]
impl ToolPort for UnusedTools {
    fn definitions(&self) -> Vec<Value> {
        Vec::new()
    }
    fn requires_permission(&self, _name: &str) -> bool {
        false
    }
    async fn execute(
        &self,
        _name: &str,
        _arguments: &Value,
        _cancel: &CancellationToken,
    ) -> Result<String> {
        anyhow::bail!("tools are not used by the oversize-request path")
    }
}

struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now(&self) -> u64 {
        1
    }
    fn id(&self) -> String {
        "1".to_owned()
    }
}
