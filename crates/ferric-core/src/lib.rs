//! ferric-core —— 纯逻辑层，无 GUI 依赖。
//!
//! 每个工具一个模块，函数保持无副作用、可单测。UI 层（ferric-ui）只负责
//! 调用这些函数并渲染结果。

pub mod crypto;
pub mod gm;
pub mod idgen;
pub mod json;
pub mod regex;
pub mod rsa;
pub mod sql;
pub mod timestamp;
pub mod yaml;
