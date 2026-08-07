//! 数据源：真服务端，或本地演示数据（mock）。
//!
//! 插件市场与自动更新原本硬绑在 `ServerProfile` 上：没烘入服务器地址的构建里，
//! 这两块界面只有一行「本构建未配置更新服务器」—— 既没法开发，也没法演示。
//!
//! 这里把「数据从哪来」抽成一层。真实分支的代码路径**一个字节都没动**
//! （见 `market::browse_server` / `updater::check_server`），演示分支是另一条独立实现。
//!
//! # 演示分支绝不碰安全边界
//!
//! 演示数据只负责「列表长什么样、下载进度怎么跳、装完状态怎么变」。它：
//! - **不写**真实插件目录（`plugin_host::install` 只接受走完验签的字节）；
//! - **不执行**任何安装程序（应用更新那一步在演示下只提示，不 `launch`）；
//! - 在界面上一律带「演示数据」标记，不会被误认成真实来源。
//!
//! 换句话说，演示模式能造出来的最坏结果就是「界面上多了几条假数据」。

use crate::net::ServerProfile;

/// 市场 / 更新的数据来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// 真服务端（走加密信道 + 离线验签的完整信任链）。
    Server(ServerProfile),
    /// 本地演示数据，不联网。
    Mock,
}

impl Source {
    /// 按「设置里的开关 + 本构建有没有烘入服务器」决定用哪个源。
    ///
    /// `want_mock`：`Some(true/false)` 是用户显式选的；`None` 表示「自动」——
    /// 没烘入服务器时自动用演示数据，否则用真服务端。这样开发构建打开就有东西看，
    /// 发行构建又不会莫名其妙显示假数据。
    pub fn resolve(want_mock: Option<bool>, server: Option<ServerProfile>) -> Option<Self> {
        match want_mock {
            Some(true) => Some(Self::Mock),
            Some(false) => server.map(Self::Server),
            None => match server {
                Some(p) => Some(Self::Server(p)),
                None => Some(Self::Mock),
            },
        }
    }

    pub fn is_mock(&self) -> bool {
        matches!(self, Self::Mock)
    }

    /// 是不是编译期烘入的那个服务器 —— 只有它才允许自动下载并执行安装包。
    pub fn is_builtin_server(&self) -> bool {
        match self {
            Self::Server(p) => p.is_builtin(),
            Self::Mock => false,
        }
    }

    /// 允许**自动**在后台下载吗？
    ///
    /// 演示源允许（反正只是模拟，且不会执行）；真服务器只有内置那个允许 ——
    /// 自定义更新源可能是用户被诱导改的，不能让它在后台悄悄拉东西下来。
    pub fn allows_auto_download(&self) -> bool {
        self.is_mock() || self.is_builtin_server()
    }

    /// 下载完之后允许真的**拉起安装程序**吗？演示源永远不行。
    pub fn allows_install(&self) -> bool {
        self.is_builtin_server()
    }

    /// 界面上用来标注来源的一句话。
    pub fn badge(&self) -> Option<String> {
        match self {
            Self::Mock => Some("演示数据 · 不联网，安装不会真的执行".to_owned()),
            Self::Server(p) if !p.is_builtin() => {
                Some(format!("自定义源 · 公钥指纹 {}", p.fingerprint()))
            }
            Self::Server(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> ServerProfile {
        ServerProfile {
            base_url: "http://example.invalid/api/v1".into(),
            pubkey: format!("04{}", "ab".repeat(64)),
        }
    }

    /// 「自动」的含义：没烘入服务器就用演示数据（开发构建打开就有东西看），
    /// 烘入了就用真的（发行构建绝不显示假数据）。
    #[test]
    fn auto_prefers_the_real_server_and_falls_back_to_mock() {
        assert_eq!(Source::resolve(None, None), Some(Source::Mock));
        assert_eq!(
            Source::resolve(None, Some(server())),
            Some(Source::Server(server()))
        );
    }

    /// 用户显式选择优先于自动判断。
    #[test]
    fn explicit_choice_wins() {
        assert_eq!(
            Source::resolve(Some(true), Some(server())),
            Some(Source::Mock)
        );
        assert_eq!(
            Source::resolve(Some(false), Some(server())),
            Some(Source::Server(server()))
        );
        // 关掉演示又没有服务器 → 没有可用数据源，界面据此显示「未配置」
        assert_eq!(Source::resolve(Some(false), None), None);
    }

    /// 演示源**绝不允许**真的执行安装包 —— 这是整个演示模式的安全前提。
    #[test]
    fn mock_never_allows_installing() {
        assert!(!Source::Mock.allows_install());
        assert!(!Source::Mock.is_builtin_server());
        assert!(Source::Mock.badge().is_some(), "演示数据必须在界面上标出来");
    }

    /// 非内置服务器不许自动后台下载（用户可能是被诱导改的更新源）。
    #[test]
    fn custom_server_is_not_trusted_for_background_download() {
        let s = Source::Server(server());
        assert!(!s.is_builtin_server());
        assert!(!s.allows_auto_download());
        assert!(!s.allows_install());
        assert!(s.badge().is_some(), "自定义源必须在界面上标出来");
    }
}
