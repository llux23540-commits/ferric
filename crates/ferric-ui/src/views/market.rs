//! 插件市场视图：浏览、安装、更新。
//!
//! 网络操作全部丢后台线程（与 `updater` 同一套「线程 + mpsc + 每帧 try_recv」模式），
//! UI 线程只负责起线程和渲染 —— SM2 标量乘法与下载绝不能阻塞主循环。
//!
//! 安装完只提示「重启生效」：`plugin_host::load_all` 只在启动时跑一次，
//! 热重载要处理 `Store` 释放、`self.active` 索引失效、`Box::leak` 的字符串等问题，
//! 留作后续。这也与示例插件文档里既有的「拷贝后重启 Ferric」说法一致。

use crate::market::{self, MarketItem};
use crate::net::ServerProfile;
use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{RichText, Ui};
use std::sync::mpsc::{Receiver, TryRecvError};

enum Msg {
    Listed(Box<Result<Vec<MarketItem>, String>>),
    /// (slug, 结果)
    Installed(String, Result<(), String>),
}

#[derive(Default)]
pub struct MarketTool {
    query: String,
    items: Vec<MarketItem>,
    /// 正在安装的 slug（同一时刻只允许一个，避免并发写同一个插件目录）
    installing: Option<String>,
    loading: bool,
    status: String,
    ok: bool,
    /// 本轮是否装过东西 —— 装过就提示重启
    changed: bool,
    rx: Option<Receiver<Msg>>,
}

impl MarketTool {
    fn profile(shared: &Shared) -> Option<ServerProfile> {
        shared.server.clone()
    }

    fn refresh(&mut self, ui: &Ui, profile: ServerProfile) {
        if self.loading || self.installing.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        let q = self.query.clone();
        std::thread::spawn(move || {
            // 先拉列表，再问哪些有更新；两步都失败则整体失败
            let r = market::browse(&profile, &q).map(|mut items| {
                if let Ok(updatable) = market::check_updates(&profile) {
                    for it in items.iter_mut() {
                        it.has_update = updatable.contains(&it.slug);
                    }
                }
                items
            });
            let _ = tx.send(Msg::Listed(Box::new(r)));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.loading = true;
        self.status = "正在获取市场列表…".to_owned();
        self.ok = true;
    }

    fn install(&mut self, ui: &Ui, profile: ServerProfile, it: &MarketItem) {
        if self.loading || self.installing.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        let (slug, ver, sha, size) = (
            it.slug.clone(),
            it.version.clone(),
            it.sha256.clone(),
            it.size,
        );
        std::thread::spawn(move || {
            let r = market::install(&profile, &slug, Some(&ver), &sha, size);
            let _ = tx.send(Msg::Installed(slug, r));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.installing = Some(it.slug.clone());
        self.status = format!("正在安装 {}…", it.name);
        self.ok = true;
    }

    fn poll(&mut self, ui: &Ui) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(Msg::Listed(r)) => {
                match *r {
                    Ok(items) => {
                        self.status = format!("共 {} 个可用插件", items.len());
                        self.ok = true;
                        self.items = items;
                    }
                    Err(e) => {
                        self.status = format!("获取失败：{e}");
                        self.ok = false;
                    }
                }
                self.loading = false;
                self.rx = None;
            }
            Ok(Msg::Installed(slug, r)) => {
                match r {
                    Ok(()) => {
                        self.status = format!("{slug} 已安装，重启 Ferric 后生效");
                        self.ok = true;
                        self.changed = true;
                        // 本地状态就地更新，省一次往返
                        if let Some(it) = self.items.iter_mut().find(|i| i.slug == slug) {
                            it.installed = Some(it.version.clone());
                            it.has_update = false;
                        }
                    }
                    Err(e) => {
                        self.status = format!("{slug} 安装失败：{e}");
                        self.ok = false;
                    }
                }
                self.installing = None;
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
            Err(TryRecvError::Disconnected) => {
                self.status = "后台任务意外中断".to_owned();
                self.ok = false;
                self.loading = false;
                self.installing = None;
                self.rx = None;
            }
        }
    }
}

impl Tool for MarketTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: "market",
            name: "插件市场",
            group: "插件",
            desc: "浏览并安装 WASM 插件（全程走加密信道，安装前校验 sha256）",
            icon: icons::BOX,
            keywords: &["plugin", "market", "插件", "市场", "扩展"],
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        self.poll(ui);
        let theme = shared.theme;

        let Some(profile) = Self::profile(shared) else {
            widgets::status_line(ui, &theme, false, "本构建未配置更新服务器，插件市场不可用");
            return;
        };
        // 自定义服务器时不禁用市场：插件跑在 wasm 沙箱里、且安装前校验 sha256，
        // 风险等级远低于原生安装包，所以这里只提示不阻断。
        if !profile.is_builtin() {
            ui.label(
                RichText::new(format!(
                    "⚠ 当前使用自定义插件源（公钥指纹 {}）",
                    profile.fingerprint()
                ))
                .size(11.0)
                .color(theme.danger),
            );
        }

        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("搜索插件名 / 标识 / 简介")
                    .desired_width(280.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let busy = self.loading || self.installing.is_some();
            ui.add_enabled_ui(!busy, |ui| {
                if widgets::ghost_button(ui, &theme, "刷新").clicked() || enter {
                    self.refresh(ui, profile.clone());
                }
            });
        });

        if !self.status.is_empty() {
            widgets::status_line(ui, &theme, self.ok, &self.status);
        }
        if self.changed {
            ui.label(
                RichText::new("已安装的插件需要重启 Ferric 才会出现在侧栏")
                    .size(11.0)
                    .color(theme.faint),
            );
        }

        ui.add_space(6.0);
        if self.items.is_empty() && !self.loading {
            ui.label(
                RichText::new("点「刷新」从服务端获取插件列表")
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
                        let busy = self.loading || self.installing.is_some();
                        ui.add_enabled_ui(!busy, |ui| {
                            if widgets::ghost_button(ui, &theme, label).clicked() {
                                want_install = Some(it.clone());
                            }
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
                self.install(ui, profile.clone(), &it);
            }
            if let Some(slug) = want_uninstall {
                // 卸载只是删本地文件，不必联网，直接同步做
                match crate::plugin_host::uninstall(&slug) {
                    Ok(()) => {
                        self.status = format!("{slug} 已卸载，重启后从侧栏消失");
                        self.ok = true;
                        self.changed = true;
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
