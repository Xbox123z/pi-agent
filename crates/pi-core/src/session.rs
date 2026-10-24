//! 会话上下文：工作目录、会话 id、运行时标志。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    /// 自动批准所有需要确认的工具（等价 --yes）。
    pub auto_yes: bool,
    /// 最大 agent 往返步数。
    pub max_steps: usize,
    /// 每次请求携带的最近消息条数。
    pub keep_recent: usize,
}

impl Session {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            id: new_session_id(),
            cwd,
            auto_yes: false,
            max_steps: 24,
            keep_recent: 40,
        }
    }
}

pub fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{:x}-{:04x}", t, fastrand_u16())
}

fn fastrand_u16() -> u16 {
    // 免依赖的小随机：混合时间戳与栈地址。
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let addr = &nanos as *const u64 as u64;
    ((nanos ^ (addr >> 4)) & 0xffff) as u16
}
