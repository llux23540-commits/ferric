//! 插件市场视图：浏览、安装、更新、卸载。
//!
//! 网络操作全部丢后台线程（与 `updater` 同一套「线程 + mpsc + 每帧 try_recv」模式），
//! UI 线程只负责起线程和渲染 —— SM2 标量乘法与下载绝不能阻塞主循环。
//!
//! # 装完立刻生效，不用重启
//!
//! 以前这里只能提示「重启 Ferric 后生效」：`plugin_host::load_all` 只在启动时跑一次。
//! 现在装完 / 卸载完会置位 `Shared::reload_plugins`，由外壳在**本视图渲染结束之后**
//! 重建插件工具（见 `FerricApp::reload_plugins`）—— 必须等到那时候，
//! 因为此刻我们正身处 `self.tools[i].ui()` 之中，谁也不能在这里改那个向量。
//!
//! # 数据源
//!
//! 真服务端与演示数据走同一套界面（见 `source::Source`）。演示数据不写插件目录，
//! 因此装完不会出现在侧栏 —— 这一点由 `market::takes_effect_in_sidebar` 决定文案，
//! 绝不含糊其辞。

use crate::market::{self, Listing, MarketItem};
use crate::source::Source;
use crate::tool::{Shared, Tool, ToolMeta};
use crate::updater::{IDLE_BEAT, PROGRESS_BEAT};
use crate::{icons, widgets};
use egui::{RichText, Ui};
use std::sync::mpsc::{Receiver, TryRecvError};

enum Msg {
    Listed(Box<Result<Listing, String>>),
    /// 安装进度（已下载, 总字节）
    Progress(u64, u64),
    /// (slug, 结果)
    Installed(String, Result<(), String>),
}

#[derive(Default)]
pub struct MarketTool {
    query: String,
    items: Vec<MarketItem>,
    /// 正在安装的 slug（同一时刻只允许一个，避免并发写同一个插件目录）
    installing: Option<String>,
    /// 当前安装的进度（已下载, 总字节）；总字节为 0 表示还没开始报
    progress: (u64, u64),
    /// 待装队列：「全部更新」一次点下来，逐个装完 —— 并发装等于并发写同一个目录
    queue: Vec<MarketItem>,
    loading: bool,
    status: String,
    ok: bool,
    /// 本轮是否装过 / 卸过东西
    changed: bool,
    /// 是否已经自动拉过一次列表。
    ///
    /// 进来就先拉：以前必须先点一次「刷新」才有内容，那一步纯属多余 ——
    /// 用户点开「插件市场」本来就是为了看列表。
    auto_refreshed: bool,
    rx: Option<Receiver<Msg>>,
}

impl MarketTool {
    fn busy(&self) -> bool {
        self.loading || self.installing.is_some()
    }

    fn refresh(&mut self, ui: &Ui, source: Source) {
        if self.busy() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        let q = self.query.clone();
        std::thread::spawn(move || {
            // 先拉列表，再问哪些有更新；两步都失败则整体失败
            let r = market::browse(&source, &q).map(|mut listing| {
                if let Ok(updatable) = market::check_updates(&source) {
                    for it in listing.items.iter_mut() {
                        it.has_update = updatable.contains(&it.slug);
                    }
                }
                listing
            });
            let _ = tx.send(Msg::Listed(Box::new(r)));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.loading = true;
        self.status = "正在获取插件列表…".to_owned();
        self.ok = true;
    }

    fn install(&mut self, ui: &Ui, source: Source, it: &MarketItem) {
        if self.busy() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        // 整条元数据一起搬到后台线程：sha256 / size / 版本 / 签名都要参与校验，
        // 拆成散字段传迟早会漏掉一个
        let item = it.clone();
        std::thread::spawn(move || {
            let slug = item.slug.clone();
            let tx2 = tx.clone();
            let ctx2 = ctx.clone();
            let r = market::install(&source, &item, &mut |done, total| {
                let _ = tx2.send(Msg::Progress(done, total));
                // 与 updater 同款节流：每收到一块数据就 request_repaint() 会把
                // 整窗拖进 30+fps 的重绘风暴（egui 没有局部重绘，为一个进度条
                // 重画整窗）。软件渲染的机器上装个插件就能让整机卡住。
                ctx2.request_repaint_after(PROGRESS_BEAT);
            });
            let _ = tx.send(Msg::Installed(slug, r));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.installing = Some(it.slug.clone());
        self.progress = (0, it.size.max(0) as u64);
        self.status = format!("正在安装 {}…", it.name);
        self.ok = true;
    }

    /// 队列里还有就接着装下一个。
    fn start_next_in_queue(&mut self, ui: &Ui, source: &Source) {
        if self.installing.is_some() {
            return;
        }
        if let Some(next) = self.queue.pop() {
            self.install(ui, source.clone(), &next);
        }
    }

    /// 每帧一次，**一次把队列排干**（与 `updater::poll` 同一写法）。
    ///
    /// 排干不是洁癖：下载线程每收到一块数据就发一条 `Progress`，`net::download_to`
    /// 的缓冲区是 64KB，实际每次 read 往往只有几 KB —— 一个接近上限（10MB）的插件
    /// 能堆出上千条。而重绘被 [`PROGRESS_BEAT`] 节流到 2fps，若每帧只取一条，
    /// 队列里剩多少条就要再等多少个醒点，最后那条 `Installed` 迟到几十秒：
    /// 界面定在某个百分比不动、按钮全禁用，看着就像装挂了。
    ///
    /// 中间那些进度值本来也没人看得见（只有最后一条会被画出来），丢掉毫无损失。
    fn poll(&mut self, ui: &Ui, shared: &mut Shared, source: &Source) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Listed(r)) => {
                    match *r {
                        Ok(listing) => {
                            let n = listing.items.len();
                            let up = listing.items.iter().filter(|i| i.has_update).count();
                            self.status = format!("共 {n} 个插件");
                            if up > 0 {
                                self.status += &format!(" · {up} 个可更新");
                            }
                            // 取不全就得说出来，别让「共 N 个」看着像全部
                            if listing.truncated {
                                self.status += "（还有更多未列出，用搜索缩小范围）";
                            }
                            self.ok = true;
                            self.items = listing.items;
                        }
                        Err(e) => {
                            self.status = format!("获取失败：{e}");
                            self.ok = false;
                        }
                    }
                    self.loading = false;
                    self.rx = None;
                    return;
                }
                Ok(Msg::Progress(done, total)) => {
                    // 只更状态，接着取下一条 —— 醒点统一在队列空了之后排
                    self.progress = (done, total.max(1));
                }
                Ok(Msg::Installed(slug, r)) => {
                    match r {
                        Ok(()) => {
                            self.ok = true;
                            self.changed = true;
                            // 本地状态就地更新，省一次往返
                            if let Some(it) = self.items.iter_mut().find(|i| i.slug == slug) {
                                it.installed = Some(it.version.clone());
                                it.has_update = false;
                            }
                            if market::takes_effect_in_sidebar(source) {
                                // 让外壳在本帧渲染结束后热加载插件 —— 这里改不了 tools
                                shared.reload_plugins = true;
                                self.status = format!("{slug} 已安装并生效");
                            } else {
                                self.status =
                                    format!("{slug} 已安装（演示数据：不会真的写入插件目录）");
                            }
                        }
                        Err(e) => {
                            self.status = format!("{slug} 安装失败：{e}");
                            self.ok = false;
                            self.queue.clear(); // 批量更新中途失败就停下，别连着报一串错
                        }
                    }
                    self.installing = None;
                    self.progress = (0, 0);
                    self.rx = None;
                    self.start_next_in_queue(ui, source);
                    return;
                }
                Err(TryRecvError::Empty) => {
                    // 队列空了。按「界面上有没有东西在变」决定下一次醒点：
                    // 装东西的时候有个百分比在动，其余情况（比如拉列表，纯网络往返）
                    // 界面**什么都没变**，不该为它重绘，走慢速兜底即可 ——
                    // 结果到达时后台线程会主动叫醒我们。
                    ui.ctx()
                        .request_repaint_after(if self.installing.is_some() {
                            PROGRESS_BEAT
                        } else {
                            IDLE_BEAT
                        });
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.status = "后台任务意外中断".to_owned();
                    self.ok = false;
                    self.loading = false;
                    self.installing = None;
                    self.queue.clear();
                    self.rx = None;
                    return;
                }
            }
        }
    }

    /// 顶部一行：搜索、刷新、全部更新。
    fn toolbar(&mut self, ui: &mut Ui, theme: &crate::theme::Theme, source: &Source) {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("搜索插件名 / 标识 / 简介")
                    .desired_width(280.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let busy = self.busy();
            ui.add_enabled_ui(!busy, |ui| {
                if widgets::ghost_button(ui, theme, "刷新").clicked() || enter {
                    self.refresh(ui, source.clone());
                }
            });
            let updatable: Vec<MarketItem> = self
                .items
                .iter()
                .filter(|i| i.has_update && !i.signature.trim().is_empty())
                .cloned()
                .collect();
            if !updatable.is_empty() {
                ui.add_enabled_ui(!busy, |ui| {
                    let n = updatable.len();
                    if widgets::primary_button(ui, theme, &format!("全部更新（{n}）")).clicked()
                    {
                        // 逐个装：并发写同一个插件目录是自找麻烦
                        self.queue = updatable;
                        self.start_next_in_queue(ui, source);
                    }
                });
            }
            // 演示是要反复做的：装完一轮之后全成了「已装最新」，
            // 没有这个按钮就只能手动去删存档才能再演一遍。
            if source.is_mock() {
                ui.add_enabled_ui(!busy, |ui| {
                    if widgets::ghost_button(ui, theme, "重置演示")
                        .on_hover_text("把演示的安装状态恢复成初始的样子")
                        .clicked()
                    {
                        market::reset_demo(source);
                        self.refresh(ui, source.clone());
                    }
                });
            }
        });
    }

    /// 安装进度条（只在装的时候出现）。
    fn progress_bar(&self, ui: &mut Ui, theme: &crate::theme::Theme) {
        let Some(slug) = &self.installing else { return };
        let (done, total) = self.progress;
        let frac = if total > 0 {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("下载 {slug} {:.0}%", frac * 100.0))
                    .size(11.5)
                    .color(theme.muted),
            );
            if !self.queue.is_empty() {
                ui.label(
                    RichText::new(format!("· 队列中还有 {} 个", self.queue.len()))
                        .size(11.0)
                        .color(theme.faint),
                );
            }
        });
        // 自绘进度条：egui 的 ProgressBar 与本应用的配色对不上
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().min(420.0), 6.0),
            egui::Sense::hover(),
        );
        let r = egui::CornerRadius::same(3);
        ui.painter().rect_filled(rect, r, theme.border);
        if frac > 0.0 {
            let mut fill = rect;
            fill.set_width(rect.width() * frac);
            ui.painter().rect_filled(fill, r, theme.accent);
        }
    }
}

impl Tool for MarketTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "market",
            name: "插件市场",
            group: "插件",
            desc: "浏览并安装 WASM 插件（全程走加密信道，安装前验离线签名并校验 sha256）",
            icon: icons::BOX,
            keywords: &["plugin", "market", "插件", "市场", "扩展"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;
        let Some(source) = shared.source.clone() else {
            widgets::status_line(
                ui,
                &theme,
                false,
                "本构建未配置插件服务器，且演示数据已关闭（可在设置里打开）",
            );
            return;
        };
        self.poll(ui, shared, &source);

        // 进来就自动拉一次，不必先点「刷新」
        if !self.auto_refreshed {
            self.auto_refreshed = true;
            self.refresh(ui, source.clone());
        }

        // 来源不是内置服务器时必须标出来（演示数据 / 自定义源）。
        // 插件跑在 wasm 沙箱里、安装前验签，风险低于原生安装包，所以只提示不阻断。
        if let Some(badge) = source.badge() {
            ui.label(
                RichText::new(format!("⚠ {badge}"))
                    .size(11.0)
                    .color(theme.danger),
            );
        }

        self.toolbar(ui, &theme, &source);

        if !self.status.is_empty() {
            widgets::status_line(ui, &theme, self.ok, &self.status);
        }
        self.progress_bar(ui, &theme);
        if self.changed && !market::takes_effect_in_sidebar(&source) {
            ui.label(
                RichText::new("演示数据不会写入插件目录，因此侧栏不会出现新工具")
                    .size(11.0)
                    .color(theme.faint),
            );
        }

        ui.add_space(6.0);
        if self.items.is_empty() && !self.loading {
            ui.label(
                RichText::new("没有匹配的插件")
                    .size(12.0)
                    .color(theme.faint),
            );
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            // 借用冲突：渲染时不能同时可变借用 self.items 与调 self.install，
            // 所以先收集要执行的动作，循环结束后再做。
            let mut want_install: Option<MarketItem> = None;
            let mut want_uninstall: Option<String> = None;
            for it in &self.items {
                widgets::card(ui, &theme, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&it.name).size(14.0).color(theme.fg));
                        ui.label(
                            RichText::new(format!("v{}", it.version))
                                .family(egui::FontFamily::Monospace)
                                .size(11.0)
                                .color(theme.faint),
                        );
                        match (&it.installed, it.has_update) {
                            (Some(v), true) => {
                                ui.label(
                                    RichText::new(format!("已装 v{v} · 可更新"))
                                        .size(11.0)
                                        .color(theme.accent),
                                );
                            }
                            (Some(v), false) => {
                                ui.label(
                                    RichText::new(if v.is_empty() {
                                        "已安装".to_owned()
                                    } else {
                                        format!("已装 v{v}")
                                    })
                                    .size(11.0)
                                    .color(theme.ok),
                                );
                            }
                            (None, _) => {}
                        }
                        // 未签名的版本装不上（`market::install` 会拒），先在这里说清楚，
                        // 别让用户点了才知道
                        if it.signature.trim().is_empty() {
                            ui.label(
                                RichText::new("未签名 · 不可安装")
                                    .size(11.0)
                                    .color(theme.danger),
                            );
                        }
                    });
                    if !it.desc.is_empty() {
                        ui.label(RichText::new(&it.desc).size(11.5).color(theme.faint));
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} · 接口 v{} · {:.1} KB · ↓{}",
                                it.slug,
                                it.api_version,
                                it.size as f32 / 1024.0,
                                it.downloads
                            ))
                            .family(egui::FontFamily::Monospace)
                            .size(10.5)
                            .color(theme.faint),
                        );
                        let label = match (&it.installed, it.has_update) {
                            (None, _) => "安装",
                            (Some(_), true) => "更新",
                            (Some(_), false) => "重新安装",
                        };
                        let busy = self.busy();
                        let installable = !it.signature.trim().is_empty();
                        ui.add_enabled_ui(!busy && installable, |ui| {
                            if widgets::ghost_button(ui, &theme, label).clicked() {
                                want_install = Some(it.clone());
                            }
                        });
                        ui.add_enabled_ui(!busy, |ui| {
                            if it.installed.is_some()
                                && widgets::ghost_button(ui, &theme, "卸载").clicked()
                            {
                                want_uninstall = Some(it.slug.clone());
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
            if let Some(it) = want_install {
                self.install(ui, source.clone(), &it);
            }
            if let Some(slug) = want_uninstall {
                // 卸载只是删本地文件，不必联网，直接同步做
                match market::uninstall(&source, &slug) {
                    Ok(()) => {
                        self.ok = true;
                        self.changed = true;
                        if market::takes_effect_in_sidebar(&source) {
                            shared.reload_plugins = true;
                            self.status = format!("{slug} 已卸载，已从侧栏移除");
                        } else {
                            self.status = format!("{slug} 已卸载（演示数据）");
                        }
                        if let Some(it) = self.items.iter_mut().find(|i| i.slug == slug) {
                            it.installed = None;
                            it.has_update = false;
                        }
                    }
                    Err(e) => {
                        self.status = format!("{slug} 卸载失败：{e}");
                        self.ok = false;
                    }
                }
            }
        });
    }

    fn show_desc(&self) -> bool {
        true
    }
}
