//! 插件市场 —— 已迁移到 Slint。
//!
//! 视图在 `ui/app.slint` 的 `MarketView`：搜索 + 插件卡片列表（安装 / 更新 /
//! 卸载 / 全部更新）。浏览、下载、验签、落盘全在 [`crate::market`]，没动过 ——
//! 插件的签名链与沙箱不受 GUI 迁移影响，**已装插件不需要重新签名**。
//!
//! # 两条不能破的规则（与 egui 版一致）
//!
//! 1. **同一时刻只装一个**。并发安装等于并发写同一个插件目录。
//!    「全部更新」是排队逐个装，不是一起冲。
//! 2. **没有签名一律拒绝**。空 `signature` = 服务端未给这个版本背书。
//!    这条判断在 [`crate::market::install`] 里，这里只负责不绕过它。

use crate::icons;
use crate::market::{self, MarketItem};
use crate::source::Source;
use crate::tool::{Tool, ToolMeta};
use std::sync::mpsc::{Receiver, TryRecvError};

/// 列表标签：`全部` / `已安装`。
///
/// 只是**视图过滤**，不影响拉取 —— 服务端一次给全量列表，
/// 「已安装」是拿本地 `installed` 字段筛出来的，切标签不发请求。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketTab {
    #[default]
    All,
    Installed,
}

impl MarketTab {
    fn from_index(i: i32) -> Self {
        // 越界一律回落到「全部」：标签是界面状态，宁可多显示也不要空列表。
        if i == 1 {
            Self::Installed
        } else {
            Self::All
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::All => 0,
            Self::Installed => 1,
        }
    }
}

enum Msg {
    Listed(Box<Result<market::Listing, String>>),
    /// 安装进度（已下载, 总字节）
    Progress(u64, u64),
    /// (slug, 结果)
    Installed(String, Result<(), String>),
}

pub struct MarketTool {
    pub query: String,
    pub items: Vec<MarketItem>,
    /// 当前标签（全部 / 已安装）。
    pub tab: MarketTab,
    /// 正在安装的 slug。同一时刻只允许一个 —— 并发写同一个插件目录会互相踩。
    pub installing: Option<String>,
    /// 当前安装进度（已下载, 总字节）；总字节 0 表示还没开始报。
    pub progress: (u64, u64),
    /// 待装队列。「全部更新」一次点下来逐个装完。
    queue: Vec<MarketItem>,
    pub loading: bool,
    pub status: String,
    pub ok: bool,
    /// 本轮是否装过 / 卸过东西 —— 外壳据此决定要不要热加载插件目录。
    pub changed: bool,
    /// 是否已经自动拉过一次列表。进来就先拉：以前必须先点「刷新」才有内容，
    /// 那一步纯属多余 —— 用户点开「插件市场」本来就是为了看列表。
    auto_refreshed: bool,
    rx: Option<Receiver<Msg>>,
}

impl Default for MarketTool {
    fn default() -> Self {
        Self {
            query: String::new(),
            items: Vec::new(),
            tab: MarketTab::default(),
            installing: None,
            progress: (0, 0),
            queue: Vec::new(),
            loading: false,
            status: String::new(),
            ok: true,
            changed: false,
            auto_refreshed: false,
            rx: None,
        }
    }
}

impl MarketTool {
    /// 进入这个工具时调一次：首次进入自动拉列表。
    pub fn on_enter(&mut self, source: Option<&Source>) {
        if self.auto_refreshed {
            return;
        }
        self.auto_refreshed = true;
        self.refresh(source);
    }

    pub fn refresh(&mut self, source: Option<&Source>) {
        let Some(src) = source else {
            self.ok = false;
            self.status = "本构建未配置插件源（设置 → 数据源）".to_owned();
            return;
        };
        if self.loading || self.installing.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let src = src.clone();
        let q = self.query.clone();
        // 网络请求绝不放 UI 线程。
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Listed(Box::new(market::browse(&src, &q))));
        });
        self.rx = Some(rx);
        self.loading = true;
        self.status = "正在获取插件列表…".to_owned();
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_owned();
    }

    /// 切标签。不发请求、不动安装队列 —— 纯粹换一个过滤条件。
    pub fn set_tab(&mut self, index: i32) {
        self.tab = MarketTab::from_index(index);
    }

    /// 一条是否命中当前搜索词（名称 / slug / 简介，忽略大小写）。
    ///
    /// 搜索词同时也发给服务端（`browse` 带 query），所以刷新回来的列表本身
    /// 已经筛过；本地这层是为了**打字即时生效** —— 不然每敲一个字都要等一趟
    /// 网络往返才看到结果。列表被服务端截断时本地筛的是这一页，
    /// 按回车重新查才是全量（状态栏那句「还有更多未取到」就是提示这件事）。
    fn hits_query(&self, item: &MarketItem) -> bool {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        item.name.to_lowercase().contains(&q)
            || item.slug.to_lowercase().contains(&q)
            || item.desc.to_lowercase().contains(&q)
    }

    /// 当前标签 + 搜索词下要显示的条目。
    pub fn visible(&self) -> Vec<&MarketItem> {
        self.items
            .iter()
            .filter(|i| self.hits_query(i))
            .filter(|i| match self.tab {
                MarketTab::All => true,
                MarketTab::Installed => i.installed.is_some(),
            })
            .collect()
    }

    /// 「全部」标签上的计数：搜索命中的总数（不是拉回来的总数 ——
    /// 标签上的数字必须等于点进去能看到的条数）。
    pub fn all_count(&self) -> usize {
        self.items.iter().filter(|i| self.hits_query(i)).count()
    }

    /// 「已安装」标签上的计数（同样受搜索词约束）。
    pub fn installed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.installed.is_some() && self.hits_query(i))
            .count()
    }

    /// 这个插件是否在待装队列里。
    ///
    /// 排队以前只写在状态栏那一句话里，一被下一条状态覆盖就没了 ——
    /// 卡片自己标出来才看得见「点过了、在等前面装完」。
    pub fn is_queued(&self, slug: &str) -> bool {
        self.queue.iter().any(|q| q.slug == slug)
    }

    /// 安装 / 更新一个插件。
    pub fn install(&mut self, source: Option<&Source>, slug: &str) {
        let Some(src) = source else { return };
        if self.installing.is_some() {
            // 已经在装了：排队，而不是拒绝 —— 用户连点两个是很自然的操作。
            if let Some(item) = self.items.iter().find(|i| i.slug == slug) {
                if !self.queue.iter().any(|q| q.slug == slug) {
                    self.queue.push(item.clone());
                    self.status =
                        format!("已排队：{}（前面还有 {} 个）", item.name, self.queue.len());
                }
            }
            return;
        }
        let Some(item) = self.items.iter().find(|i| i.slug == slug).cloned() else {
            return;
        };
        self.start_install(src, item);
    }

    /// 「全部更新」：把有更新的排成队列，逐个装。
    pub fn update_all(&mut self, source: Option<&Source>) {
        let Some(src) = source else { return };
        let pending: Vec<MarketItem> = self
            .items
            .iter()
            .filter(|i| i.has_update)
            .cloned()
            .collect();
        if pending.is_empty() {
            self.status = "没有可更新的插件".to_owned();
            return;
        }
        let n = pending.len();
        self.queue = pending;
        self.status = format!("准备更新 {n} 个插件");
        if self.installing.is_none() {
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.start_install(src, next);
            }
        }
    }

    fn start_install(&mut self, src: &Source, item: MarketItem) {
        let (tx, rx) = std::sync::mpsc::channel();
        let src = src.clone();
        let slug = item.slug.clone();
        let name = item.name.clone();
        let tx2 = tx.clone();
        // slug 要在闭包外还用一次，先克隆出来 —— item 整体被搬进线程。
        let installing = item.slug.clone();
        std::thread::spawn(move || {
            let r = market::install(&src, &item, &mut |done, total| {
                let _ = tx2.send(Msg::Progress(done, total));
            });
            let _ = tx.send(Msg::Installed(slug, r));
        });
        self.rx = Some(rx);
        self.installing = Some(installing);
        self.progress = (0, 0);
        self.status = format!("正在安装 {name}…");
    }

    pub fn uninstall(&mut self, source: Option<&Source>, slug: &str) {
        let Some(src) = source else { return };
        match market::uninstall(src, slug) {
            Ok(()) => {
                self.changed = true;
                self.ok = true;
                self.status = "已卸载".to_owned();
                // 就地更新本地状态，不必等下一次 refresh —— 卸完立刻看到变化。
                if let Some(i) = self.items.iter_mut().find(|i| i.slug == slug) {
                    i.installed = None;
                    i.has_update = false;
                }
            }
            Err(e) => {
                self.ok = false;
                self.status = format!("卸载失败：{e}");
            }
        }
    }

    /// 收后台消息。返回 `true` 表示状态有变化（调用方据此刷 UI）。
    pub fn poll(&mut self, source: Option<&Source>) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut dirty = false;
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress(done, total)) => {
                    self.progress = (done, total);
                    dirty = true;
                }
                Ok(Msg::Listed(r)) => {
                    self.loading = false;
                    self.rx = None;
                    match *r {
                        Ok(listing) => {
                            self.items = listing.items;
                            self.ok = true;
                            // 取不全必须说出来。以前写死 page=1&size=100，
                            // 服务端超过 100 个插件时后面的直接消失，界面上还
                            // 理直气壮写着「共 100 个插件」—— 用户没有任何线索
                            // 知道自己看到的是残缺的一份。
                            // 措辞要说清这是「取回来多少」，不是「现在显示多少」：
                            // 标签上还有一组 `全部 N / 已安装 M` 的计数，搜索一收窄
                            // 就会和这句对不上，读者只能以为哪个数字错了。
                            self.status = if listing.truncated {
                                format!(
                                    "已取到 {} 个（还有更多未取到 —— 用搜索缩小范围）",
                                    self.items.len()
                                )
                            } else {
                                format!("源里共 {} 个插件", self.items.len())
                            };
                        }
                        Err(e) => {
                            self.ok = false;
                            self.status = format!("获取失败：{e}");
                        }
                    }
                    return true;
                }
                Ok(Msg::Installed(slug, r)) => {
                    self.installing = None;
                    self.rx = None;
                    self.progress = (0, 0);
                    match r {
                        Ok(()) => {
                            self.changed = true;
                            self.ok = true;
                            self.status = "安装完成".to_owned();
                            if let Some(i) = self.items.iter_mut().find(|i| i.slug == slug) {
                                i.installed = Some(i.version.clone());
                                i.has_update = false;
                            }
                        }
                        Err(e) => {
                            self.ok = false;
                            self.status = format!("安装失败：{e}");
                        }
                    }
                    // 队列里还有就接着装（**逐个**，不并发）。
                    if let (Some(src), Some(next)) = (source, self.queue.first().cloned()) {
                        self.queue.remove(0);
                        self.start_install(src, next);
                    }
                    return true;
                }
                Err(TryRecvError::Empty) => return dirty,
                Err(TryRecvError::Disconnected) => {
                    self.loading = false;
                    self.installing = None;
                    self.rx = None;
                    self.ok = false;
                    self.status = "后台线程意外中断".to_owned();
                    return true;
                }
            }
        }
    }

    /// 进度百分比（0–100）。总字节未知时返回 0。
    pub fn progress_pct(&self) -> i32 {
        let (done, total) = self.progress;
        if total == 0 {
            return 0;
        }
        ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as i32
    }

    pub fn pending_updates(&self) -> usize {
        self.items.iter().filter(|i| i.has_update).count()
    }

    /// 取走「本轮有改动」标记（外壳据此热加载插件目录）。
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

impl Tool for MarketTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "market",
            name: "插件市场",
            desc: "浏览并安装 WASM 插件，全部更新，安装包一律验签。",
            icon: icons::BOX,
            group: "插件",
            keywords: &["plugin", "market", "插件", "市场", "扩展"],
        }
    }

    // 市场不持久化草稿：列表是服务端状态，搜索词存下来只会让下次进来
    // 看到一个被过滤过的列表却不知道为什么。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Source {
        Source::Mock
    }

    fn wait_idle(t: &mut MarketTool, src: &Source) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while (t.loading || t.installing.is_some()) && std::time::Instant::now() < deadline {
            if !t.poll(Some(src)) {
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
    }

    /// 一条列表项。只填过滤用得上的字段，其余给占位值。
    fn item(slug: &str, name: &str, desc: &str, installed: Option<&str>) -> MarketItem {
        MarketItem {
            slug: slug.to_owned(),
            name: name.to_owned(),
            desc: desc.to_owned(),
            version: "1.0.0".to_owned(),
            api_version: 1,
            size: 1024,
            sha256: String::new(),
            signature: "x".to_owned(),
            downloads: 7,
            has_update: installed.is_some_and(|v| v != "1.0.0"),
            installed: installed.map(str::to_owned),
        }
    }

    /// 三条：一条未装、一条已装最新、一条已装旧版。不走网络，过滤是纯本地的。
    fn filled() -> MarketTool {
        MarketTool {
            items: vec![
                item("json-lint", "JSON 校验", "检查 JSON 语法", None),
                item("csv-tool", "CSV 工具", "CSV 转换", Some("1.0.0")),
                item("hash-kit", "哈希工具箱", "常见摘要算法", Some("0.9.0")),
            ],
            ..MarketTool::default()
        }
    }

    #[test]
    fn entering_the_tool_fetches_the_list_once() {
        // 以前必须先点「刷新」才有内容 —— 那一步纯属多余。
        let mut t = MarketTool::default();
        let src = demo();
        t.on_enter(Some(&src));
        assert!(t.loading, "进入时应当立刻开始拉列表");
        wait_idle(&mut t, &src);
        assert!(t.ok, "{}", t.status);
        assert!(!t.items.is_empty(), "演示源应当返回插件列表");

        // 第二次进入不再重复拉
        t.on_enter(Some(&src));
        assert!(!t.loading);
    }

    #[test]
    fn no_source_is_reported_not_silently_empty() {
        let mut t = MarketTool::default();
        t.refresh(None);
        assert!(!t.ok);
        assert!(t.status.contains("未配置"), "{}", t.status);
    }

    #[test]
    fn install_marks_the_item_and_flags_change() {
        let mut t = MarketTool::default();
        let src = demo();
        t.on_enter(Some(&src));
        wait_idle(&mut t, &src);

        let slug = t.items[0].slug.clone();
        t.install(Some(&src), &slug);
        assert_eq!(t.installing.as_deref(), Some(slug.as_str()));
        wait_idle(&mut t, &src);

        assert!(t.ok, "{}", t.status);
        let item = t.items.iter().find(|i| i.slug == slug).unwrap();
        assert!(item.installed.is_some(), "装完应当就地标记为已安装");
        assert!(t.take_changed(), "装完必须报告有改动（外壳据此热加载）");
        assert!(!t.take_changed(), "标记应当只报一次");
    }

    #[test]
    fn a_second_install_request_queues_instead_of_racing() {
        // 并发安装等于并发写同一个插件目录。
        let mut t = MarketTool::default();
        let src = demo();
        t.on_enter(Some(&src));
        wait_idle(&mut t, &src);
        if t.items.len() < 2 {
            return; // 演示源只有一个插件时这条无从验证
        }

        let a = t.items[0].slug.clone();
        let b = t.items[1].slug.clone();
        t.install(Some(&src), &a);
        t.install(Some(&src), &b);
        assert_eq!(t.installing.as_deref(), Some(a.as_str()), "第一个仍在装");
        assert!(t.status.contains("已排队"), "{}", t.status);
        // 卡片上的「排队中」徽标靠这个：只写在状态栏里的话，
        // 下一条状态一覆盖就看不出这个插件已经点过了。
        assert!(t.is_queued(&b), "第二个应当在队列里");
        assert!(!t.is_queued(&a), "正在装的那个不算排队");
        // 切标签不该动安装状态（标签只是视图过滤）
        t.set_tab(1);
        assert_eq!(t.installing.as_deref(), Some(a.as_str()));
        assert!(t.is_queued(&b));
    }

    #[test]
    fn update_all_with_nothing_to_do_says_so() {
        let mut t = MarketTool::default();
        let src = demo();
        t.on_enter(Some(&src));
        wait_idle(&mut t, &src);
        if t.pending_updates() > 0 {
            return; // 演示源恰好有待更新时这条不适用
        }
        t.update_all(Some(&src));
        assert!(t.status.contains("没有可更新"), "{}", t.status);
    }

    #[test]
    fn progress_percent_is_clamped_and_safe_at_zero_total() {
        let mut t = MarketTool::default();
        assert_eq!(t.progress_pct(), 0, "总字节未知时不该除零");
        t.progress = (50, 100);
        assert_eq!(t.progress_pct(), 50);
        // 服务端报了个比总数还大的已下载值也不该越界
        t.progress = (999, 100);
        assert_eq!(t.progress_pct(), 100);
    }

    #[test]
    fn uninstall_updates_local_state_immediately() {
        // 不就地更新的话，卸完要等下一次 refresh 才看到变化。
        let mut t = MarketTool::default();
        let src = demo();
        t.on_enter(Some(&src));
        wait_idle(&mut t, &src);

        let slug = t.items[0].slug.clone();
        t.install(Some(&src), &slug);
        wait_idle(&mut t, &src);
        let _ = t.take_changed();

        t.uninstall(Some(&src), &slug);
        assert!(t.ok, "{}", t.status);
        let item = t.items.iter().find(|i| i.slug == slug).unwrap();
        assert!(item.installed.is_none(), "卸完应当立刻显示为未安装");
        assert!(t.take_changed());
    }

    #[test]
    fn poll_without_pending_work_is_a_noop() {
        let mut t = MarketTool::default();
        assert!(!t.poll(Some(&demo())));
    }

    #[test]
    fn market_does_not_persist_a_draft() {
        // 存搜索词只会让下次进来看到一个被过滤的列表却不知道为什么。
        assert_eq!(MarketTool::default().save_draft(), None);
    }

    #[test]
    fn installed_tab_shows_only_installed_plugins() {
        let mut t = filled();
        let names: Vec<&str> = t.visible().iter().map(|i| i.slug.as_str()).collect();
        assert_eq!(
            names,
            ["json-lint", "csv-tool", "hash-kit"],
            "全部标签给全量"
        );

        t.set_tab(1);
        let names: Vec<&str> = t.visible().iter().map(|i| i.slug.as_str()).collect();
        assert_eq!(names, ["csv-tool", "hash-kit"], "已安装标签只留装过的");

        // 越界的下标不能变成空列表（界面给不出，但 API 挡得住）
        t.set_tab(9);
        assert_eq!(t.visible().len(), 3);
    }

    #[test]
    fn tag_counts_follow_the_search_word() {
        // 标签上的数字必须等于点进去看到的条数，否则「已安装 2」点进去
        // 只有 1 条会让人以为界面坏了。
        let mut t = filled();
        assert_eq!(t.all_count(), 3);
        assert_eq!(t.installed_count(), 2);

        t.set_query("csv");
        assert_eq!(t.all_count(), 1);
        assert_eq!(t.installed_count(), 1);
        assert_eq!(t.visible().len(), t.all_count());

        t.set_tab(1);
        assert_eq!(t.visible().len(), t.installed_count());
    }

    #[test]
    fn search_filters_locally_across_name_slug_and_desc() {
        // 搜索词也发给服务端，但本地这层让打字即时生效（不用等网络往返）。
        let mut t = filled();
        t.set_query("哈希");
        assert_eq!(t.visible()[0].slug, "hash-kit", "按名称命中");
        t.set_query("HASH-KIT");
        assert_eq!(t.visible().len(), 1, "按 slug 命中且忽略大小写");
        t.set_query("摘要");
        assert_eq!(t.visible().len(), 1, "按简介命中");
        t.set_query("  ");
        assert_eq!(t.visible().len(), 3, "全是空白等于没搜");
        t.set_query("不存在的东西");
        assert!(t.visible().is_empty());
    }

    #[test]
    fn search_and_installed_tab_apply_together() {
        let mut t = filled();
        t.set_tab(1);
        t.set_query("json");
        // json-lint 命中搜索但没装：已安装标签下就该是空的
        assert!(t.visible().is_empty(), "两个条件是与，不是或");
        assert_eq!(t.all_count(), 1, "同一个搜索词在「全部」下有 1 条");
    }
}
