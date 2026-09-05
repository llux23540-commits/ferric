//! UUID 生成器（v4 / v7 / v5 命名 / v6）—— 已完整迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `UuidView`；这里只有状态与业务。生成逻辑全在
//! `ferric_core::idgen`（无 GUI 依赖，带单测），迁 GUI 框架时完全没动过。
//!
//! 输出走**只读编辑区**（[`crate::editor::TextBuffer`] + `CodeEditor`）：
//! 鼠标能拖选任意一段、双击选整行、Ctrl+C 复制，跟其它工具的输出面板一致。
//! 中间做过一版「点一行高亮一行」——那只能整行选，用户要的是拖着选。

use crate::editor::TextBuffer;
use crate::icons;
use crate::tool::{Tool, ToolMeta};
use ferric_core::idgen::{self, IdKind, Namespace, Opts};
use serde::{Deserialize, Serialize};

/// 执行记录保留条数的可选档位（对应 Slint 里的 Segmented 索引）。
pub const KEEP_OPTS: [i64; 4] = [3, 5, 10, 20];

fn default_hist_keep() -> i64 {
    3
}

/// 草稿格式与 egui 版**逐字段一致** —— 老用户升级后 UUID 工具的设置不会丢。
#[derive(Serialize, Deserialize)]
struct UuidDraft {
    kind: IdKind,
    count: i64,
    namespace: Namespace,
    custom_ns: String,
    name: String,
    upper: bool,
    nohyphen: bool,
    as_json: bool,
    #[serde(default = "default_hist_keep")]
    hist_keep: i64,
}

pub struct HistEntry {
    pub label: String,
    pub body: String,
}

pub struct UuidTool {
    pub kind: IdKind,
    pub count: i64,
    pub namespace: Namespace,
    pub custom_ns: String,
    pub name: String,
    pub upper: bool,
    pub nohyphen: bool,
    pub as_json: bool,
    /// 输出的纯文本（历史、整体复制、导出都用它）。
    pub output: String,
    /// 界面上那个只读编辑区的缓冲 —— 鼠标拖选 / Ctrl+C 走它。
    /// 与 `output` 同源，只由 [`UuidTool::set_output`] 一处写。
    pub out: TextBuffer,
    pub ok: bool,
    pub status: String,
    pub history: Vec<HistEntry>,
    counter: u32,
    /// 执行记录保留条数（3/5/10/20 档位，随草稿持久化）。
    pub hist_keep: i64,
}

impl Default for UuidTool {
    fn default() -> Self {
        let mut t = Self {
            kind: IdKind::UuidV4,
            count: 10,
            namespace: Namespace::Dns,
            custom_ns: String::new(),
            name: "example.com".to_owned(),
            upper: false,
            nohyphen: false,
            as_json: false,
            output: String::new(),
            out: TextBuffer::default(),
            ok: true,
            status: "就绪".to_owned(),
            history: Vec::new(),
            counter: 0,
            hist_keep: default_hist_keep(),
        };
        t.regen();
        t
    }
}

impl UuidTool {
    fn opts(&self) -> Opts<'_> {
        Opts {
            kind: self.kind,
            count: self.count.clamp(1, 1000) as usize,
            namespace: self.namespace,
            custom_ns: &self.custom_ns,
            name: &self.name,
            upper: self.upper,
            nohyphen: self.nohyphen,
            as_json: self.as_json,
        }
    }

    /// 输出的唯一写入口：纯文本与只读编辑区的缓冲一起更新。
    ///
    /// 分成两处存是因为两边的用途不同：`output` 给历史 / 整体复制 / 导出，
    /// `out` 给界面（拖选、滚动、Ctrl+C）。**只在这里写**，免得两边漂开。
    fn set_output(&mut self, text: String) {
        self.out.set_text(&text);
        self.out.clear_dirty();
        self.output = text;
    }

    /// 重新生成。失败（如自定义命名空间非法）时不覆盖输出、不记历史，只报状态。
    pub fn regen(&mut self) {
        match idgen::generate(&self.opts()) {
            Ok(items) => {
                let text = if self.as_json {
                    serde_json::to_string_pretty(&items).unwrap_or_default()
                } else {
                    items.join("\n")
                };
                self.set_output(text);
                self.ok = true;
                self.status = format!("已生成 {} 条", items.len());
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("生成失败：{e}");
                return;
            }
        }
        self.counter = self.counter.wrapping_add(1);
        // history[0] 是当前这次，其后保留最近 hist_keep 次
        let label = format!(
            "{} · {} 个 · #{}",
            self.kind.label(),
            self.count.clamp(1, 1000),
            self.counter
        );
        self.history.insert(
            0,
            HistEntry {
                label,
                body: self.output.clone(),
            },
        );
        self.history
            .truncate(1 + self.hist_keep.clamp(1, 20) as usize);
    }

    /// 把某条历史恢复成当前输出（不重新生成 —— 用户要的就是那一次的结果）。
    pub fn restore(&mut self, idx: usize) -> bool {
        match self
            .history
            .get(idx)
            .map(|h| (h.body.clone(), h.label.clone()))
        {
            Some((body, label)) => {
                self.set_output(body);
                self.status = format!("已恢复：{label}");
                self.ok = true;
                true
            }
            None => false,
        }
    }

    // ——— Slint 侧用索引表达枚举，这里做双向映射 ———

    pub fn kind_index(&self) -> i32 {
        IdKind::ALL
            .iter()
            .position(|k| *k == self.kind)
            .unwrap_or(0) as i32
    }

    pub fn set_kind_index(&mut self, i: i32) {
        if let Some(k) = IdKind::ALL.get(i.max(0) as usize) {
            self.kind = *k;
        }
    }

    pub fn namespace_index(&self) -> i32 {
        Namespace::ALL
            .iter()
            .position(|n| *n == self.namespace)
            .unwrap_or(0) as i32
    }

    pub fn set_namespace_index(&mut self, i: i32) {
        if let Some(n) = Namespace::ALL.get(i.max(0) as usize) {
            self.namespace = *n;
        }
    }

    pub fn hist_keep_index(&self) -> i32 {
        KEEP_OPTS
            .iter()
            .position(|k| *k == self.hist_keep)
            .unwrap_or(0) as i32
    }

    pub fn set_hist_keep_index(&mut self, i: i32) {
        if let Some(k) = KEEP_OPTS.get(i.max(0) as usize) {
            self.hist_keep = *k;
            // 档位调小时立刻裁掉多余记录，别让界面显示比设置更多的行。
            self.history
                .truncate(1 + self.hist_keep.clamp(1, 20) as usize);
        }
    }
}

impl Tool for UuidTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "uuid",
            name: "UUID 生成器",
            desc: "UUID v4 / v7 / v6 / v5（命名空间），大小写 / 无连字符，Raw / JSON，执行历史",
            icon: icons::CREDIT_CARD,
            group: "生成",
            keywords: &["uuid", "guid", "v4", "v5", "v7", "生成", "标识符"],
        }
    }

    fn save_draft(&self) -> Option<String> {
        serde_json::to_string(&UuidDraft {
            kind: self.kind,
            count: self.count,
            namespace: self.namespace,
            custom_ns: self.custom_ns.clone(),
            name: self.name.clone(),
            upper: self.upper,
            nohyphen: self.nohyphen,
            as_json: self.as_json,
            hist_keep: self.hist_keep,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<UuidDraft>(data) {
            self.kind = d.kind;
            self.count = d.count.clamp(1, 1000);
            self.namespace = d.namespace;
            self.custom_ns = d.custom_ns;
            self.name = d.name;
            self.upper = d.upper;
            self.nohyphen = d.nohyphen;
            self.as_json = d.as_json;
            self.hist_keep = d.hist_keep.clamp(1, 20);
            // 恢复草稿后按恢复出来的参数重算一次，界面上不留空输出。
            self.regen();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_roundtrip_preserves_every_field() {
        let t = UuidTool {
            kind: IdKind::UuidV5,
            count: 42,
            namespace: Namespace::Url,
            custom_ns: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".into(),
            name: "ferric.dev".into(),
            upper: true,
            nohyphen: true,
            as_json: true,
            hist_keep: 20,
            ..Default::default()
        };

        let saved = t.save_draft().expect("UUID 工具必须持久化草稿");
        let mut restored = UuidTool::default();
        restored.load_draft(&saved);

        assert_eq!(restored.kind, IdKind::UuidV5);
        assert_eq!(restored.count, 42);
        assert_eq!(restored.namespace, Namespace::Url);
        assert_eq!(restored.custom_ns, "6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        assert_eq!(restored.name, "ferric.dev");
        assert!(restored.upper);
        assert!(restored.nohyphen);
        assert!(restored.as_json);
        assert_eq!(restored.hist_keep, 20);
    }

    #[test]
    fn count_out_of_range_is_clamped_on_load() {
        let mut t = UuidTool::default();
        // 手工构造越界草稿（外部文件可写，必须防住）
        let bad = r#"{"kind":"UuidV4","count":99999,"namespace":"Dns","custom_ns":"","name":"x","upper":false,"nohyphen":false,"as_json":false,"hist_keep":999}"#;
        t.load_draft(bad);
        assert_eq!(t.count, 1000, "数量上限必须夹到 1000");
        assert_eq!(t.hist_keep, 20, "历史档位上限必须夹到 20");
    }

    #[test]
    fn shrinking_history_keep_trims_existing_entries() {
        let mut t = UuidTool {
            hist_keep: 20,
            ..Default::default()
        };
        for _ in 0..12 {
            t.regen();
        }
        assert!(t.history.len() > 4, "先攒够记录才能验证裁剪");
        // 档位调到最小（3）应立刻裁到 1 + 3
        t.set_hist_keep_index(0);
        assert_eq!(t.history.len(), 4);
    }

    #[test]
    fn restore_puts_that_run_back_into_output() {
        let mut t = UuidTool::default();
        t.regen();
        t.regen();
        let want = t.history[1].body.clone();
        assert!(t.restore(1));
        assert_eq!(t.output, want);
    }

    #[test]
    fn invalid_custom_namespace_reports_error_without_clobbering_output() {
        let mut t = UuidTool::default();
        let good = t.output.clone();
        t.kind = IdKind::UuidV5;
        t.namespace = Namespace::Custom;
        t.custom_ns = "not-a-uuid".into();
        t.regen();
        assert!(!t.ok, "非法命名空间必须报错");
        assert_eq!(t.output, good, "失败时不能把上一次的好输出冲掉");
    }

    #[test]
    fn enum_index_mapping_is_bidirectional() {
        let mut t = UuidTool::default();
        for (i, kind) in IdKind::ALL.iter().enumerate() {
            t.set_kind_index(i as i32);
            assert_eq!(t.kind, *kind);
            assert_eq!(t.kind_index(), i as i32);
        }
        for (i, ns) in Namespace::ALL.iter().enumerate() {
            t.set_namespace_index(i as i32);
            assert_eq!(t.namespace, *ns);
            assert_eq!(t.namespace_index(), i as i32);
        }
    }

    #[test]
    fn out_of_range_index_is_ignored_not_panicking() {
        let mut t = UuidTool::default();
        t.set_kind_index(99);
        t.set_namespace_index(-3);
        t.set_hist_keep_index(1234);
        // 越界索引一律忽略，保持原值
        assert_eq!(t.kind, IdKind::UuidV4);
        assert_eq!(t.namespace, Namespace::Dns);
        assert_eq!(t.hist_keep, 3);
    }

    #[test]
    fn output_lands_in_the_selectable_buffer() {
        // 输出要进只读编辑区才能用鼠标拖选 —— 两处必须同源。
        let mut t = UuidTool {
            count: 3,
            ..Default::default()
        };
        t.regen();
        assert_eq!(t.out.text(), t.output);
        assert_eq!(t.out.total_lines(), 3);

        // 恢复历史走同一条写入口
        t.regen();
        assert!(t.restore(1), "样例里应当有第二条历史");
        assert_eq!(t.out.text(), t.output);
    }

    #[test]
    fn dragging_selects_part_of_one_id() {
        // 用户要的是「拖着选」：从第 2 行第 3 列拖到第 9 列，
        // 拿到的就是那一段，而不是整行、也不是整篇。
        let mut t = UuidTool {
            count: 3,
            ..Default::default()
        };
        t.regen();
        t.out.set_viewport(10, 80);
        t.out.click(1, 3.0, false);
        t.out.click(1, 9.0, true); // 拖动 = extend

        let line = t.output.lines().nth(1).expect("第二行");
        assert_eq!(t.out.selected_text(), line[3..9]);
        assert!(t.out.has_selection());

        // 双击整行：这也是「单条选中」的入口
        t.out.select_line();
        assert_eq!(t.out.selected_text(), line);
    }

    #[test]
    fn the_output_pane_is_read_only() {
        // 输出面板不该能被改：改了也会被下一次生成冲掉，只会让人困惑。
        // 只读由外壳按 `-out` 后缀判定（见 `wire_editors`），这里守住前提：
        // 缓冲区标识必须是 `uuid-out`。
        let mut t = UuidTool {
            count: 1,
            ..Default::default()
        };
        t.regen();
        let before = t.out.text();
        let outcome = crate::editor_bridge::apply_key(&mut t.out, "X", false, false, true);
        assert!(!outcome.edited);
        assert_eq!(t.out.text(), before);
    }

    #[test]
    fn a_thousand_ids_stay_virtualized() {
        // 崩溃守门：Slint 原生 TextEdit 在约 2190 行以上 panic，
        // 输出面板必须走虚拟化的那条路（只渲染一屏）。
        let mut t = UuidTool {
            count: 1000,
            ..Default::default()
        };
        t.regen();
        t.out.set_viewport(24, 80);
        assert_eq!(t.out.total_lines(), 1000);
        assert_eq!(t.out.visible_lines().len(), 24);
    }
}
