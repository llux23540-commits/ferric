//! 数据源：真服务端，或本地演示数据（mock）。
//!
//! 插件市场与自动更新原本硬绑在 `ServerProfile` 上：没烘入服务器地址的构建里，
//! 这两块界面只有一行「本构建未配置更新服务器」—— 既没法开发，也没法演示。
//!
//! 这里把「数据从哪来」抽成一层。真实分支（见 `market::browse_server` /
//! `updater::check_server`）上的每一处校验都还在原地，演示分支是另一条独立实现 ——
//! 两条路自始至终没有共用过任何「校验」代码，演示也就无从削弱它。
//!
//! # 演示分支绝不碰安全边界
//!
//! 演示数据只负责「列表长什么样、下载进度怎么跳、装完状态怎么变」。它：
//! - **不写**真实插件目录（`plugin_host::install` 只接受走完验签的字节）；
//! - **不执行**任何安装程序（应用更新那一步在演示下只提示，不 `launch`）；
//! - 在界面上一律带「演示数据」标记，不会被误认成真实来源。
//!
//! 换句话说，演示模式能造出来的最坏结果就是「界面上多了几条假数据」。

use crate::github::GithubSource;
use crate::net::ServerProfile;

/// 市场 / 更新的数据来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// 真服务端（走加密信道 + 离线验签的完整信任链）。
    Server(ServerProfile),
    /// GitHub Releases（只做应用更新，**没有插件市场**）。
    ///
    /// 传输层交给 GitHub 的 TLS，执行授权仍然只认离线签名 —— 信任根没变。
    /// 详见 [`crate::github`]。
    Github(GithubSource),
    /// 本地演示数据，不联网。
    Mock,
}

/// 用户在设置里选的数据源。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourcePref {
    #[default]
    Auto,
    Server,
    Github,
    Mock,
}

impl SourcePref {
    /// 从旧版本的 `Option<bool>`（None=自动 / false=服务器 / true=演示）迁移。
    pub fn from_legacy(v: Option<bool>) -> Self {
        match v {
            None => Self::Auto,
            Some(false) => Self::Server,
            Some(true) => Self::Mock,
        }
    }

    /// 回写成旧字段，好让「装回旧版本」的用户不至于把设置丢光。
    /// GitHub 是旧版本没有的概念，只能落回「自动」。
    pub fn to_legacy(self) -> Option<bool> {
        match self {
            Self::Auto | Self::Github => None,
            Self::Server => Some(false),
            Self::Mock => Some(true),
        }
    }
}

impl Source {
    /// 按「设置里选的 + 本构建烘入了什么」决定用哪个源。
    ///
    /// 「自动」的优先级是**自建服务端 > GitHub > 演示**，这条顺序对应着产品阶段：
    /// 烘入了 `FERRIC_SERVER_URL` 就说明这个构建已经有自己的服务（要卖插件、要授权），
    /// 更新自然也该走自己的；还没到那一步就用 GitHub 发布页（免服务器、免备案，
    /// 但只有更新、没有市场）；两者都没有才用演示数据 —— 开发构建打开就有东西看。
    ///
    /// ⚠️ 迁移顺序上有个陷阱：**源是编译期烘进二进制的**，已经装在用户机器上的旧
    /// 客户端不会因为你上线了服务端就自己切过去。正确的次序是「先用 GitHub 把带
    /// 新服务端地址的客户端推给所有人，再把服务端当主源」。
    pub fn resolve(
        pref: SourcePref,
        server: Option<ServerProfile>,
        github: Option<GithubSource>,
    ) -> Option<Self> {
        match pref {
            SourcePref::Mock => Some(Self::Mock),
            SourcePref::Server => server.map(Self::Server),
            SourcePref::Github => github.map(Self::Github),
            SourcePref::Auto => server
                .map(Self::Server)
                .or_else(|| github.map(Self::Github))
                .or(Some(Self::Mock)),
        }
    }

    pub fn is_mock(&self) -> bool {
        matches!(self, Self::Mock)
    }

    /// 这个来源是不是**编译期烘入**的那一个。
    ///
    /// 「烘入」= 写死在二进制里，改它必须重新编译并重新分发；而 `app.ron` 是
    /// 当前用户可写的，任何同用户进程都能静默改写。两者的可信度差着一个数量级，
    /// 所以自动下载与自动安装只认前者。
    pub fn is_builtin(&self) -> bool {
        match self {
            Self::Server(p) => p.is_builtin(),
            Self::Github(g) => g.is_builtin(),
            Self::Mock => false,
        }
    }

    /// 允许**自动**在后台下载吗？
    ///
    /// 演示源允许（反正只是模拟，且不会执行）；真服务器只有内置那个允许 ——
    /// 自定义更新源可能是用户被诱导改的，不能让它在后台悄悄拉东西下来。
    pub fn allows_auto_download(&self) -> bool {
        self.is_mock() || self.is_builtin()
    }

    /// 下载完之后允许真的**拉起安装程序**吗？演示源永远不行。
    ///
    /// 注意：即便是用户自己填的来源，包也必须过离线签名那一关才可能走到这里 ——
    /// 换句话说这条规则拦的不是「恶意的包」（签名已经拦掉了），而是
    /// 「**静默改配置 + 静默自动安装**」这条链路。配置文件可被同用户进程改写，
    /// 所以非烘入来源一律降级为「只提示，手动装」。
    pub fn allows_install(&self) -> bool {
        self.is_builtin()
    }

    /// 界面上用来标注来源的一句话。
    pub fn badge(&self) -> Option<String> {
        match self {
            Self::Mock => Some("演示数据 · 不联网，安装不会真的执行".to_owned()),
            Self::Server(p) if !p.is_builtin() => {
                Some(format!("自定义源 · 公钥指纹 {}", p.fingerprint()))
            }
            Self::Server(_) => None,
            Self::Github(g) if !g.is_builtin() => {
                Some(format!("自定义 {} · 仅提示新版本，不自动安装", g.label()))
            }
            Self::Github(g) => Some(format!("{} · 无插件市场", g.label())),
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

    fn gh() -> GithubSource {
        GithubSource {
            repo: "owner/ferric".into(),
        }
    }

    /// 「自动」的优先级顺序 = 产品阶段：自建服务端 > GitHub > 演示。
    ///
    /// 商业化之前只烘 GitHub，更新走发布页；商业化之后烘上服务端，它自动接管。
    #[test]
    fn auto_follows_the_product_stage() {
        use SourcePref::Auto;
        // 什么都没烘：开发构建，用演示数据（打开就有东西看）
        assert_eq!(Source::resolve(Auto, None, None), Some(Source::Mock));
        // 只烘了 GitHub：商业化之前
        assert_eq!(
            Source::resolve(Auto, None, Some(gh())),
            Some(Source::Github(gh()))
        );
        // 两个都烘了：自建服务端接管（它才有市场与授权）
        assert_eq!(
            Source::resolve(Auto, Some(server()), Some(gh())),
            Some(Source::Server(server()))
        );
    }

    /// 用户显式选择优先于自动判断；选了但那个源没烘入 → 没有可用源。
    #[test]
    fn explicit_choice_wins() {
        assert_eq!(
            Source::resolve(SourcePref::Mock, Some(server()), Some(gh())),
            Some(Source::Mock)
        );
        assert_eq!(
            Source::resolve(SourcePref::Server, Some(server()), Some(gh())),
            Some(Source::Server(server()))
        );
        assert_eq!(
            Source::resolve(SourcePref::Github, Some(server()), Some(gh())),
            Some(Source::Github(gh()))
        );
        assert_eq!(Source::resolve(SourcePref::Server, None, Some(gh())), None);
        assert_eq!(
            Source::resolve(SourcePref::Github, Some(server()), None),
            None
        );
    }

    /// 旧配置文件里的 `Option<bool>` 要能平滑迁移，且回写不丢基本设置。
    #[test]
    fn legacy_preference_migrates_both_ways() {
        assert_eq!(SourcePref::from_legacy(None), SourcePref::Auto);
        assert_eq!(SourcePref::from_legacy(Some(false)), SourcePref::Server);
        assert_eq!(SourcePref::from_legacy(Some(true)), SourcePref::Mock);
        for p in [SourcePref::Auto, SourcePref::Server, SourcePref::Mock] {
            assert_eq!(
                SourcePref::from_legacy(p.to_legacy()),
                p,
                "{p:?} 往返应稳定"
            );
        }
        // 旧版本没有 GitHub 这个概念，只能落回「自动」——但绝不能落成「演示」，
        // 那会让降级用户莫名其妙看到假数据
        assert_eq!(SourcePref::Github.to_legacy(), None);
    }

    /// 演示源**绝不允许**真的执行安装包 —— 这是整个演示模式的安全前提。
    #[test]
    fn mock_never_allows_installing() {
        assert!(!Source::Mock.allows_install());
        assert!(!Source::Mock.is_builtin());
        assert!(Source::Mock.badge().is_some(), "演示数据必须在界面上标出来");
    }

    /// 非内置来源不许自动后台下载（用户可能是被诱导改的更新源）。
    /// 自建服务器与 GitHub 一视同仁 —— 差别只在「是不是烘进二进制的」。
    #[test]
    fn custom_sources_are_not_trusted_for_background_download() {
        for s in [Source::Server(server()), Source::Github(gh())] {
            assert!(!s.is_builtin(), "{s:?} 不是烘入的");
            assert!(!s.allows_auto_download(), "{s:?} 不该自动下载");
            assert!(!s.allows_install(), "{s:?} 不该自动安装");
            assert!(s.badge().is_some(), "{s:?} 必须在界面上标出来");
        }
    }
}
