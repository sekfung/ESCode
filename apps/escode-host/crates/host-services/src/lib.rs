//! Host 服务域（rust-u2-host-skeleton.md §5 优先序）：按域拆分，逐域交付。
//! 当前域：settings（唯一写者，与 TS Host 同文件同格式）。
pub mod settings;
