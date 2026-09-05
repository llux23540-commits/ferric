//! RSA 密钥对生成 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `RsaView`：位数分段 + 生成按钮 + 两个只读 PEM 面板。
//! 生成逻辑在 `ferric_core::rsa`，没动过。
//!
//! 生成 4096 位要跑好几秒，所以放后台线程 —— UI 线程绝不做大数运算。
//! 结果由外壳用 100ms 定时器取（[`RsaTool::poll`]）。egui 时代这里要
//! `request_repaint_after`，Slint 是 retained mode，property 一变就重画脏区域，
//! 定时器只负责「去看看线程有没有结果」。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::rsa;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{Receiver, TryRecvError};

/// 可选位数档位（与 egui 版一致）。
pub const BITS_OPTS: [i64; 4] = [1024, 2048, 3072, 4096];

#[derive(Serialize, Deserialize)]
struct RsaDraft {
    bits: i64,
}

pub struct RsaTool {
    pub bits: i64,
    /// 公钥 / 私钥都用虚拟化编辑区：4096 位私钥的 PEM 有几十行，
    /// 而且用户会想选中一段复制 —— 只读缓冲区正好。
    pub pub_pem: TextBuffer,
    pub priv_pem: TextBuffer,
    pub status: String,
    pub ok: bool,
    pub busy: bool,
    rx: Option<Receiver<Result<(String, String), String>>>,
}

impl Default for RsaTool {
    fn default() -> Self {
        // 刻意**不在构造时就生成**：应用启动时会构造全部工具，那会让每次启动
        // 都白跑一次 2048 位密钥生成（几百毫秒到数秒）。egui 版在这里调了
        // regen()，代价是启动时后台线程立刻开始算一份用户可能根本不看的密钥。
        Self {
            bits: 2048,
            pub_pem: TextBuffer::new(""),
            priv_pem: TextBuffer::new(""),
            status: "就绪 —— 点「生成」开始".to_owned(),
            ok: true,
            busy: false,
            rx: None,
        }
    }
}

impl RsaTool {
    /// 起一个后台线程生成密钥对。重复点击会被 `busy` 拦住 ——
    /// 并发生成除了烧 CPU 没有任何意义。
    pub fn regen(&mut self) {
        if self.busy {
            return;
        }
        let bits = self.bits.clamp(256, 4096) as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(rsa::generate(bits));
        });
        self.rx = Some(rx);
        self.busy = true;
        self.status = format!("生成中… {bits} 位");
    }

    /// 取一次后台结果。返回 `true` 表示状态有变化（调用方据此刷 UI）。
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(Ok((p, s))) => {
                self.pub_pem.set_text(&p);
                self.priv_pem.set_text(&s);
                self.status = format!("已生成 {} 位", self.bits);
                self.ok = true;
                self.busy = false;
                self.rx = None;
                true
            }
            Ok(Err(e)) => {
                self.status = format!("生成失败：{e}");
                self.ok = false;
                self.busy = false;
                self.rx = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.status = "生成线程意外中断".to_owned();
                self.ok = false;
                self.busy = false;
                self.rx = None;
                true
            }
        }
    }

    pub fn bits_index(&self) -> i32 {
        BITS_OPTS.iter().position(|b| *b == self.bits).unwrap_or(1) as i32
    }

    /// 换位数。**不自动生成** —— 用户可能只是想看看有哪些档位，
    /// 自动开跑会白烧几秒 CPU。
    pub fn set_bits_index(&mut self, i: i32) {
        if let Some(b) = BITS_OPTS.get(i.max(0) as usize) {
            self.bits = *b;
            if !self.busy {
                self.status = format!("已选 {b} 位 —— 点「生成」开始");
            }
        }
    }
}

impl Tool for RsaTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "rsa",
            name: "RSA 密钥对",
            desc: "1024–4096 位，后台线程生成，PEM 输出。",
            icon: icons::KEY,
            group: "生成",
            keywords: &["rsa", "key", "密钥", "pem", "公钥", "私钥"],
        }
    }

    /// 只存位数。**绝不持久化私钥** —— `app.ron` 是当前用户可写的普通文件，
    /// 把私钥写进去等于给任何同用户进程留一份。
    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&RsaDraft { bits: self.bits }).ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<RsaDraft>(data) {
            self.bits = d.bits.clamp(256, 4096);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_up_does_not_generate_anything() {
        // 启动时会构造全部工具。在这里生成密钥等于每次开应用都白烧几秒 CPU。
        let t = RsaTool::default();
        assert!(!t.busy);
        assert!(t.pub_pem.is_empty());
        assert!(t.priv_pem.is_empty());
    }

    #[test]
    fn generate_produces_a_pem_pair() {
        let mut t = RsaTool::default();
        t.set_bits_index(0); // 1024 位，测试里够快
        t.regen();
        assert!(t.busy);

        // 等后台线程（1024 位通常几十到几百毫秒；给足余量）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while t.busy && std::time::Instant::now() < deadline {
            if !t.poll() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        assert!(!t.busy, "生成没有在 60 秒内完成");
        assert!(t.ok, "{}", t.status);
        assert!(
            t.pub_pem.text().contains("BEGIN PUBLIC KEY"),
            "公钥不是 PEM：{}",
            t.pub_pem.text()
        );
        assert!(
            t.priv_pem.text().contains("PRIVATE KEY"),
            "私钥不是 PEM：{}",
            t.priv_pem.text()
        );
        assert!(t.priv_pem.total_lines() > 1, "PEM 应当是多行");
    }

    #[test]
    fn clicking_generate_twice_does_not_start_two_threads() {
        // 并发生成除了烧 CPU 没有任何意义。
        let mut t = RsaTool::default();
        t.set_bits_index(0);
        t.regen();
        let status = t.status.clone();
        t.regen();
        assert_eq!(t.status, status, "第二次点击不该改变状态");
    }

    #[test]
    fn poll_without_a_pending_job_is_a_noop() {
        let mut t = RsaTool::default();
        assert!(!t.poll());
    }

    #[test]
    fn changing_bits_does_not_kick_off_generation() {
        let mut t = RsaTool::default();
        t.set_bits_index(3);
        assert_eq!(t.bits, 4096);
        assert!(!t.busy, "换档位不该自动开跑");
    }

    #[test]
    fn draft_never_contains_the_private_key() {
        // app.ron 是当前用户可写的普通文件。私钥进去就等于泄露给同用户的任何进程。
        let mut t = RsaTool::default();
        t.priv_pem
            .set_text("-----BEGIN PRIVATE KEY-----\nSECRET\n-----END PRIVATE KEY-----");
        t.pub_pem
            .set_text("-----BEGIN PUBLIC KEY-----\nPUB\n-----END PUBLIC KEY-----");
        let saved = t.save_draft().expect("必须持久化位数");
        assert!(!saved.contains("SECRET"), "草稿里出现了私钥内容：{saved}");
        assert!(!saved.contains("PRIVATE"), "草稿里出现了私钥字样：{saved}");
        assert!(!saved.contains("PUB"), "草稿里连公钥也不该存：{saved}");
    }

    #[test]
    fn draft_roundtrip_preserves_bits() {
        let mut t = RsaTool::default();
        t.set_bits_index(2);
        let saved = t.save_draft().unwrap();
        let mut r = RsaTool::default();
        r.load_draft(&saved);
        assert_eq!(r.bits, 3072);
        assert_eq!(r.bits_index(), 2);
    }

    #[test]
    fn out_of_range_bits_in_draft_are_clamped() {
        let mut t = RsaTool::default();
        t.load_draft(r#"{"bits":999999}"#);
        assert_eq!(t.bits, 4096, "位数上限必须夹住 —— 文件是用户可写的");
        t.load_draft(r#"{"bits":1}"#);
        assert_eq!(t.bits, 256);
    }
}
