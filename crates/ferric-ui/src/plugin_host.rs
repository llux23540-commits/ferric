//! WASM 插件宿主。
//!
//! 扫描 `%APPDATA%/ferric/plugins/*.wasm`，按固定 ABI 加载为 [`PluginTool`]
//! 并追加进工具注册表。安全边界：
//! - wasmtime 沙箱：插件默认无文件 / 网络 / 系统调用能力，只能纯计算；
//! - 燃料限额：单次调用指令数封顶，死循环只会让本次调用失败；
//! - 内存上限：线性内存 64MB 封顶，I/O JSON 8MB 封顶；
//! - 任何加载 / 调用失败都以错误信息呈现，绝不影响宿主本体。
//!
//! ABI（数据一律 UTF-8 JSON，(ptr,len) 打包为 i64：高 32 位 ptr、低 32 位 len）：
//! - `ferric_alloc(len: i32) -> ptr: i32`   宿主借插件内存写入数据
//! - `ferric_dealloc(ptr: i32, len: i32)`   归还缓冲区
//! - `ferric_manifest() -> i64`             返回 Manifest JSON
//! - `ferric_process(ptr: i32, len: i32) -> i64`  ProcessIn JSON → ProcessOut JSON

use crate::tool::{Shared, Tool, ToolMeta};
use crate::{icons, widgets};
use egui::{RichText, TextEdit, Ui};
use ferric_core::plugin::{Manifest, OptionSpec, ProcessIn, ProcessOut};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wasmtime::{
    Config, Engine, Instance, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

/// 单次调用燃料上限（≈ 数亿条指令；常规文本处理远用不满，死循环会被掐断）。
const CALL_FUEL: u64 = 2_000_000_000;
/// 插件线性内存上限。
const MEM_LIMIT: usize = 64 * 1024 * 1024;
/// 单次调用输入 / 输出 JSON 大小上限。
const IO_LIMIT: usize = 8 * 1024 * 1024;

struct HostState {
    limits: StoreLimits,
}

fn friendly_trap(e: wasmtime::Error) -> String {
    let s = e.to_string();
    if s.contains("fuel") {
        "插件执行超时（燃料耗尽，可能存在死循环）".to_owned()
    } else {
        format!("插件运行错误：{s}")
    }
}

fn unpack(ret: i64) -> (usize, usize) {
    let v = ret as u64;
    ((v >> 32) as usize, (v & 0xFFFF_FFFF) as usize)
}

/// 从插件内存拷出 packed(ptr,len) 指向的字节，并调用 dealloc 归还。
fn take_packed(
    store: &mut Store<HostState>,
    memory: &Memory,
    dealloc: &TypedFunc<(i32, i32), ()>,
    ret: i64,
) -> Result<Vec<u8>, String> {
    let (ptr, len) = unpack(ret);
    if len > IO_LIMIT {
        return Err(format!("插件输出过大（{len} 字节，上限 {IO_LIMIT}）"));
    }
    let data = memory.data(&mut *store);
    let end = ptr.checked_add(len).ok_or("插件返回的指针越界")?;
    if end > data.len() {
        return Err("插件返回的指针越界".into());
    }
    let out = data[ptr..end].to_vec();
    if len > 0 {
        dealloc
            .call(&mut *store, (ptr as i32, len as i32))
            .map_err(friendly_trap)?;
    }
    Ok(out)
}

/// 已实例化的 WASM 插件。
struct WasmPlugin {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    process: TypedFunc<(i32, i32), i64>,
}

impl WasmPlugin {
    fn load(engine: &Engine, path: &Path) -> Result<(Self, Manifest), String> {
        let module = Module::from_file(engine, path).map_err(|e| format!("模块解析失败：{e}"))?;
        let mut store = Store::new(
            engine,
            HostState {
                limits: StoreLimitsBuilder::new().memory_size(MEM_LIMIT).build(),
            },
        );
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(CALL_FUEL)
            .map_err(|e| format!("燃料初始化失败：{e}"))?;

        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| format!("实例化失败（插件不得依赖 WASI 等外部导入）：{e}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("插件未导出线性内存 `memory`")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "ferric_alloc")
            .map_err(|_| "缺少导出函数 ferric_alloc(i32)->i32")?;
        let dealloc = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "ferric_dealloc")
            .map_err(|_| "缺少导出函数 ferric_dealloc(i32,i32)")?;
        let process = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "ferric_process")
            .map_err(|_| "缺少导出函数 ferric_process(i32,i32)->i64")?;
        let manifest_fn = instance
            .get_typed_func::<(), i64>(&mut store, "ferric_manifest")
            .map_err(|_| "缺少导出函数 ferric_manifest()->i64")?;

        let ret = manifest_fn.call(&mut store, ()).map_err(friendly_trap)?;
        let raw = take_packed(&mut store, &memory, &dealloc, ret)?;
        let manifest: Manifest =
            serde_json::from_slice(&raw).map_err(|e| format!("manifest JSON 非法：{e}"))?;
        manifest.validate()?;

        Ok((
            WasmPlugin {
                store,
                memory,
                alloc,
                dealloc,
                process,
            },
            manifest,
        ))
    }

    /// 一次 process 调用：JSON 入 → JSON 出。
    fn call_process(&mut self, in_json: &str) -> Result<String, String> {
        let bytes = in_json.as_bytes();
        if bytes.len() > IO_LIMIT {
            return Err(format!("输入过大（上限 {IO_LIMIT} 字节）"));
        }
        self.store
            .set_fuel(CALL_FUEL)
            .map_err(|e| format!("燃料重置失败：{e}"))?;

        // 借插件内存写入输入
        let in_len = bytes.len() as i32;
        let in_ptr = self
            .alloc
            .call(&mut self.store, in_len)
            .map_err(friendly_trap)?;
        {
            let data = self.memory.data_mut(&mut self.store);
            let start = in_ptr as usize;
            let end = start
                .checked_add(bytes.len())
                .ok_or("插件 alloc 返回的指针越界")?;
            if in_ptr < 0 || end > data.len() {
                return Err("插件 alloc 返回的指针越界".into());
            }
            data[start..end].copy_from_slice(bytes);
        }

        let ret = self
            .process
            .call(&mut self.store, (in_ptr, in_len))
            .map_err(friendly_trap)?;
        // 归还输入缓冲区（process 借用但不拥有）
        self.dealloc
            .call(&mut self.store, (in_ptr, in_len))
            .map_err(friendly_trap)?;

        let out = take_packed(&mut self.store, &self.memory, &self.dealloc, ret)?;
        String::from_utf8(out).map_err(|_| "插件输出不是有效 UTF-8".into())
    }
}

/// 选项运行时状态（与 manifest.options 一一对应）。
enum OptState {
    Seg(usize),
    Toggle(bool),
    Text(String),
}

impl OptState {
    fn value(&self, spec: &OptionSpec) -> String {
        match (self, spec) {
            (OptState::Seg(i), OptionSpec::Seg { values, .. }) => {
                values.get(*i).cloned().unwrap_or_default()
            }
            (OptState::Toggle(b), _) => b.to_string(),
            (OptState::Text(s), _) => s.clone(),
            _ => String::new(),
        }
    }
}

/// 把 WASM 插件包装成常规工具：按 manifest 渲染表单，实时调用 process。
pub struct PluginTool {
    plugin: WasmPlugin,
    manifest: Manifest,
    // ToolMeta 要求 'static 引用：插件常驻进程生命周期，加载时一次性 leak。
    st_id: &'static str,
    st_name: &'static str,
    st_group: &'static str,
    st_desc: &'static str,
    st_keywords: &'static [&'static str],
    opts: Vec<OptState>,
    input: String,
    output: String,
    ok: bool,
    status: String,
    dirty: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PluginDraft {
    input: String,
    #[serde(default)]
    options: BTreeMap<String, String>,
}

impl PluginTool {
    fn new(plugin: WasmPlugin, manifest: Manifest) -> Self {
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        let st_id = leak(format!("ext-{}", manifest.id));
        let st_name = leak(manifest.name.clone());
        let st_group = leak(manifest.group.clone());
        let st_desc = leak(if manifest.desc.is_empty() {
            format!("{}（WASM 插件）", manifest.name)
        } else {
            format!("{}（WASM 插件）", manifest.desc)
        });
        let kw: Vec<&'static str> = manifest.keywords.iter().map(|k| leak(k.clone())).collect();
        let st_keywords: &'static [&'static str] = Box::leak(kw.into_boxed_slice());
        let opts = manifest
            .options
            .iter()
            .map(|o| match o {
                OptionSpec::Seg { default, .. } => OptState::Seg(*default),
                OptionSpec::Toggle { default, .. } => OptState::Toggle(*default),
                OptionSpec::Text { default, .. } => OptState::Text(default.clone()),
            })
            .collect();
        Self {
            plugin,
            manifest,
            st_id,
            st_name,
            st_group,
            st_desc,
            st_keywords,
            opts,
            input: String::new(),
            output: String::new(),
            ok: true,
            status: "就绪".to_owned(),
            dirty: true,
        }
    }

    fn plugin_id(&self) -> &str {
        &self.manifest.id
    }

    fn run(&mut self) {
        let mut options = BTreeMap::new();
        for (spec, st) in self.manifest.options.iter().zip(&self.opts) {
            options.insert(spec.key().to_owned(), st.value(spec));
        }
        let pin = ProcessIn {
            input: self.input.clone(),
            options,
        };
        let in_json = match serde_json::to_string(&pin) {
            Ok(s) => s,
            Err(e) => {
                self.ok = false;
                self.status = format!("入参编码失败：{e}");
                return;
            }
        };
        match self.plugin.call_process(&in_json).and_then(|s| {
            serde_json::from_str::<ProcessOut>(&s).map_err(|e| format!("插件出参非法：{e}"))
        }) {
            Ok(out) if out.ok => {
                self.output = out.output;
                self.ok = true;
                self.status = "完成".to_owned();
            }
            Ok(out) => {
                self.ok = false;
                self.status = if out.error.is_empty() {
                    "插件返回失败".to_owned()
                } else {
                    out.error
                };
            }
            Err(e) => {
                self.ok = false;
                self.status = e;
            }
        }
    }
}

impl Tool for PluginTool {
    fn meta(&self) -> ToolMeta {
        ToolMeta {
            id: self.st_id,
            name: self.st_name,
            group: self.st_group,
            desc: self.st_desc,
            icon: icons::BOX,
            keywords: self.st_keywords,
        }
    }

    fn ui(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let theme = shared.theme;

        // 选项行（按 manifest 渲染）
        if !self.manifest.options.is_empty() {
            let specs = self.manifest.options.clone();
            ui.horizontal_wrapped(|ui| {
                for (i, spec) in specs.iter().enumerate() {
                    match (spec, &mut self.opts[i]) {
                        (OptionSpec::Seg { label, values, .. }, OptState::Seg(sel)) => {
                            widgets::field_label(ui, &theme, label);
                            ui.add_space(4.0);
                            let vals: Vec<&str> = values.iter().map(|s| s.as_str()).collect();
                            if let Some(n) = widgets::seg(ui, &theme, &vals, *sel) {
                                *sel = n;
                                self.dirty = true;
                            }
                        }
                        (OptionSpec::Toggle { label, .. }, OptState::Toggle(on)) => {
                            if widgets::pill_toggle(ui, &theme, *on, label) {
                                *on = !*on;
                                self.dirty = true;
                            }
                        }
                        (OptionSpec::Text { label, hint, .. }, OptState::Text(text)) => {
                            widgets::field_label(ui, &theme, label);
                            ui.add_space(4.0);
                            if ui
                                .add(
                                    TextEdit::singleline(text)
                                        .desired_width(180.0)
                                        .hint_text(hint.as_str()),
                                )
                                .changed()
                            {
                                self.dirty = true;
                            }
                        }
                        _ => {}
                    }
                    ui.add_space(10.0);
                }
            });
            ui.add_space(10.0);
        }

        // 输入 / 输出双栏
        let in_label = self
            .manifest
            .input_label
            .clone()
            .unwrap_or_else(|| "输入".to_owned());
        let out_label = self
            .manifest
            .output_label
            .clone()
            .unwrap_or_else(|| "输出".to_owned());
        let out_lines = self.output.lines().count();
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                widgets::panel(
                    ui,
                    &theme,
                    &in_label,
                    |_ui| {},
                    |ui| {
                        if widgets::code_area(ui, self.st_id, &mut self.input, true, 14).changed() {
                            self.dirty = true;
                        }
                    },
                );
            });
            cols[1].vertical(|ui| {
                widgets::panel(
                    ui,
                    &theme,
                    &out_label,
                    |ui| {
                        ui.label(
                            RichText::new(format!("{out_lines} 行"))
                                .size(11.0)
                                .color(theme.faint),
                        );
                    },
                    |ui| {
                        let id = format!("{}-out", self.st_id);
                        widgets::code_area(ui, &id, &mut self.output, false, 14);
                    },
                );
            });
        });

        if self.dirty {
            self.dirty = false;
            self.run();
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::subtle_button(ui, &theme, Some(icons::COPY), "复制输出").clicked() {
                shared.copy(ui.ctx(), self.output.clone());
            }
            ui.add_space(6.0);
            widgets::status_line(ui, &theme, self.ok, &self.status);
        });
    }

    fn save_draft(&self) -> Option<String> {
        let mut options = BTreeMap::new();
        for (spec, st) in self.manifest.options.iter().zip(&self.opts) {
            options.insert(spec.key().to_owned(), st.value(spec));
        }
        serde_json::to_string(&PluginDraft {
            input: self.input.clone(),
            options,
        })
        .ok()
    }

    fn load_draft(&mut self, data: &str) {
        if let Ok(d) = serde_json::from_str::<PluginDraft>(data) {
            self.input = d.input;
            for (i, spec) in self.manifest.options.clone().iter().enumerate() {
                let Some(v) = d.options.get(spec.key()) else {
                    continue;
                };
                match (spec, &mut self.opts[i]) {
                    (OptionSpec::Seg { values, .. }, OptState::Seg(sel)) => {
                        if let Some(n) = values.iter().position(|x| x == v) {
                            *sel = n;
                        }
                    }
                    (_, OptState::Toggle(on)) => *on = v == "true",
                    (_, OptState::Text(t)) => *t = v.clone(),
                    _ => {}
                }
            }
            self.dirty = true;
        }
    }
}

/// 插件目录：`%APPDATA%/ferric/plugins`（与 eframe 存储同级）。
pub fn plugins_dir() -> Option<PathBuf> {
    let pd = directories::ProjectDirs::from("", "", "ferric")?;
    let base = pd
        .data_dir()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| pd.data_dir().to_path_buf());
    Some(base.join("plugins"))
}

/// 扫描插件目录并加载全部 .wasm 插件；返回 (工具, 加载警告)。
/// 目录不存在则创建后返回空——首次运行即准备好放置插件的位置。
pub fn load_all() -> (Vec<PluginTool>, Vec<String>) {
    let mut tools: Vec<PluginTool> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let Some(dir) = plugins_dir() else {
        return (tools, warns);
    };
    if !dir.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
        return (tools, warns);
    }

    let mut cfg = Config::new();
    cfg.consume_fuel(true);
    let engine = match Engine::new(&cfg) {
        Ok(e) => e,
        Err(e) => {
            warns.push(format!("WASM 引擎初始化失败：{e}"));
            return (tools, warns);
        }
    };

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("wasm"))
        })
        .collect();
    paths.sort();

    for path in paths {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match WasmPlugin::load(&engine, &path) {
            Ok((p, m)) => {
                if tools.iter().any(|t| t.plugin_id() == m.id) {
                    warns.push(format!("{file}: 插件 id「{}」重复，已跳过", m.id));
                    continue;
                }
                tools.push(PluginTool::new(p, m));
            }
            Err(e) => warns.push(format!("{file}: {e}")),
        }
    }
    (tools, warns)
}
