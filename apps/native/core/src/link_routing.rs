//! Offline routing for the desktop's external link entry points.

use url::Url;

use crate::sessions::InterfaceLocale;

const RELEASE_HOST: &str = "github.com";
const RELEASE_PATH: &str = "/icesword0760/mantle-browser/releases";
pub const BROWSER_RELEASES_URL: &str =
    "https://github.com/icesword0760/mantle-browser/releases/latest";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource<'a> {
    External,
    /// The URL from the trusted provider preset selected by the caller.
    ProviderKeyPage {
        key_url: &'a str,
    },
    SignIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRejection {
    Missing,
    Malformed,
    UnsupportedScheme,
    UntrustedDestination,
}

impl LinkRejection {
    pub fn message(self, locale: InterfaceLocale) -> &'static str {
        locale.text("不允许打开此外部链接", "External URL is not allowed")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserLink<'a> {
    url: &'a str,
    source: LinkSource<'a>,
}

impl<'a> BrowserLink<'a> {
    /// Open this URL in the controlled browser first.
    pub fn url(self) -> &'a str {
        self.url
    }

    /// The provider key page exposes the address for manual opening or copying.
    pub fn copy_fallback(
        self,
        failure: OpenFailure,
        locale: InterfaceLocale,
    ) -> Option<CopyFallback<'a>> {
        if !matches!(self.source, LinkSource::ProviderKeyPage { .. }) {
            return None;
        }
        let prefix = match failure {
            OpenFailure::BridgeUnavailable => {
                locale.text("请在浏览器打开：", "Open in your browser: ")
            }
            OpenFailure::BrowserDidNotOpen => {
                locale.text("浏览器未打开，请访问：", "Browser did not open. Visit: ")
            }
        };
        Some(CopyFallback {
            url: self.url,
            prefix,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailure {
    BridgeUnavailable,
    BrowserDidNotOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyFallback<'a> {
    pub url: &'a str,
    pub prefix: &'static str,
}

impl CopyFallback<'_> {
    pub fn message(self) -> String {
        format!("{}{}", self.prefix, self.url)
    }
}

pub fn route_link<'a>(
    value: Option<&'a str>,
    source: LinkSource<'a>,
) -> Result<BrowserLink<'a>, LinkRejection> {
    let raw = value.ok_or(LinkRejection::Missing)?;
    if raw.chars().any(char::is_control) {
        return Err(LinkRejection::Malformed);
    }
    let url = raw.trim_matches(' ');
    if url.is_empty() {
        return Err(LinkRejection::Missing);
    }
    let parsed = Url::parse(url).map_err(|_| LinkRejection::Malformed)?;
    if parsed.scheme() != "https" {
        return Err(LinkRejection::UnsupportedScheme);
    }
    if has_credentials(&parsed) || parsed.port().is_some() {
        return Err(LinkRejection::UntrustedDestination);
    }
    let allowed = is_external_destination(&parsed)
        || match source {
            LinkSource::External => false,
            LinkSource::ProviderKeyPage { key_url } => raw == key_url,
            LinkSource::SignIn => is_sign_in_destination(&parsed),
        };
    if !allowed {
        return Err(LinkRejection::UntrustedDestination);
    }
    Ok(BrowserLink { url, source })
}

fn has_credentials(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some_and(|value| !value.is_empty())
}

fn is_external_destination(url: &Url) -> bool {
    match url.host_str() {
        Some("ip.net.coffee" | "fuck-claude.vercel.app") => true,
        Some(RELEASE_HOST) => url
            .path()
            .strip_prefix(RELEASE_PATH)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
        _ => false,
    }
}

fn is_sign_in_destination(url: &Url) -> bool {
    if url.fragment().is_some_and(|fragment| !fragment.is_empty()) {
        return false;
    }
    let (port, path) = match (url.host_str(), url.path()) {
        (Some("auth.openai.com"), "/oauth/authorize" | "/api/accounts/authorize") => {
            (1455, "/auth/callback")
        }
        (Some("accounts.google.com"), "/o/oauth2/v2/auth" | "/o/oauth2/auth") => {
            (51121, "/oauth-callback")
        }
        _ => return false,
    };
    url.query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .and_then(|(_, value)| Url::parse(&value).ok())
        .is_some_and(|redirect| {
            redirect.scheme() == "http"
                && matches!(
                    redirect.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]")
                )
                && redirect.port() == Some(port)
                && redirect.path() == path
                && !has_credentials(&redirect)
                && redirect.query().is_none_or(str::is_empty)
                && redirect.fragment().is_none_or(str::is_empty)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledBrowserFailure {
    Missing,
    LaunchFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationAction {
    InstallBrowser,
    OpenSystemBrowser,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationButton {
    pub action: ConfirmationAction,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemBrowserConfirmation {
    pub title: &'static str,
    pub message: &'static str,
    pub detail: Option<&'static str>,
    pub buttons: Vec<ConfirmationButton>,
    pub default_index: usize,
    pub cancel_index: usize,
}

impl SystemBrowserConfirmation {
    /// Only a selected button can request a system open. Dismissal opens nothing.
    pub fn selected_action(&self, response: Option<usize>) -> ConfirmationAction {
        response
            .and_then(|index| self.buttons.get(index))
            .map_or(ConfirmationAction::Cancel, |button| button.action)
    }
}

pub fn system_browser_confirmation(
    failure: ControlledBrowserFailure,
    locale: InterfaceLocale,
) -> SystemBrowserConfirmation {
    use ConfirmationAction::{Cancel, InstallBrowser, OpenSystemBrowser};

    let system = ConfirmationButton {
        action: OpenSystemBrowser,
        label: locale.text("用系统浏览器打开", "Open in system browser"),
    };
    let cancel = ConfirmationButton {
        action: Cancel,
        label: locale.text("取消", "Cancel"),
    };
    match failure {
        ControlledBrowserFailure::Missing => SystemBrowserConfirmation {
            title: locale.text("Mantle 未安装", "Mantle is not installed"),
            message: locale.text(
                "从 Matou 打开的网页默认使用 Mantle 浏览器。",
                "Matou opens web pages in the Mantle browser by default.",
            ),
            detail: Some(locale.text(
                "安装 Mantle 后，登录和网页都会在受控环境里打开。",
                "After you install Mantle, sign-in and web pages will open in a controlled browser.",
            )),
            buttons: vec![
                ConfirmationButton {
                    action: InstallBrowser,
                    label: locale.text("安装 Mantle", "Install Mantle"),
                },
                system,
                cancel,
            ],
            default_index: 0,
            cancel_index: 2,
        },
        ControlledBrowserFailure::LaunchFailed => SystemBrowserConfirmation {
            title: locale.text("Mantle 无法打开", "Mantle could not open"),
            message: locale.text(
                "这次没能用 Mantle 打开网页。",
                "Mantle could not open this page.",
            ),
            detail: None,
            buttons: vec![system, cancel],
            default_index: 0,
            // The frozen dialog maps Escape to index 0.
            cancel_index: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InterfaceLocale::{Chinese, English};
    use LinkRejection::{Malformed, Missing, UnsupportedScheme, UntrustedDestination};
    use LinkSource::{External, ProviderKeyPage, SignIn};

    #[test]
    fn external_https_uses_exact_hosts_and_release_paths() {
        for value in [
            "HTTPS://IP.NET.COFFEE:443/",
            "https://fuck-claude.vercel.app/",
            " https://ip.net.coffee/ ",
            BROWSER_RELEASES_URL,
            "https://github.com/icesword0760/mantle-browser/releases",
            "https://github.com/icesword0760/mantle-browser/releases/tag/v148.1",
        ] {
            assert_eq!(
                route_link(Some(value), External).unwrap().url(),
                value.trim()
            );
        }
        for value in [
            "https://ip.net.coffee.invalid/",
            "https://sub.ip.net.coffee/",
            "https://ip.net.coffee./",
            "https://user@ip.net.coffee/",
            "https://:password@ip.net.coffee/",
            "https://ip.net.coffee:444/",
            "https://example.invalid/",
            "https://github.com/icesword0760/mantle-browser/releases-extra",
            "https://github.com/icesword0760/mantle-browser/releases/%2e%2e/other",
            "https://github.com/icesword0760/mantle-browser",
        ] {
            assert_eq!(
                route_link(Some(value), External),
                Err(UntrustedDestination),
                "{value}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_schemes_and_invalid_inputs() {
        for value in [
            "http://ip.net.coffee/",
            "mailto:example@example.invalid",
            "file:///tmp/file",
            "JaVaScRiPt:void(0)",
            "custom+app://open",
            "data:text/plain,hello",
            "https-custom://ip.net.coffee/",
        ] {
            assert_eq!(route_link(Some(value), External), Err(UnsupportedScheme));
        }
        for value in [
            "not a URL",
            "//ip.net.coffee/",
            "https://",
            "ht\ttps://ip.net.coffee/",
            "\nhttps://ip.net.coffee/",
            "https://ip.net.coffee/\u{7f}",
            "https://ip.net.coffee/\0",
        ] {
            assert_eq!(
                route_link(Some(value), External),
                Err(Malformed),
                "{value:?}"
            );
        }
        assert_eq!(route_link(None, External), Err(Missing));
        assert_eq!(route_link(Some(""), External), Err(Missing));
        assert_eq!(route_link(Some(" "), External), Err(Missing));
        assert_eq!(Malformed.message(Chinese), "不允许打开此外部链接");
        assert_eq!(Malformed.message(English), "External URL is not allowed");
    }

    #[test]
    fn only_selected_preset_can_expose_the_frozen_manual_fallback() {
        let value = "https://platform.deepseek.com/api_keys";
        let preset = ProviderKeyPage { key_url: value };
        assert_eq!(route_link(Some(value), External), Err(UntrustedDestination));
        assert_eq!(
            route_link(Some("https://platform.deepseek.com/api_keys/other"), preset),
            Err(UntrustedDestination)
        );
        let link = route_link(Some(value), preset).unwrap();
        for (failure, locale, prefix) in [
            (OpenFailure::BridgeUnavailable, Chinese, "请在浏览器打开："),
            (
                OpenFailure::BridgeUnavailable,
                English,
                "Open in your browser: ",
            ),
            (
                OpenFailure::BrowserDidNotOpen,
                Chinese,
                "浏览器未打开，请访问：",
            ),
            (
                OpenFailure::BrowserDidNotOpen,
                English,
                "Browser did not open. Visit: ",
            ),
        ] {
            let fallback = link.copy_fallback(failure, locale).unwrap();
            assert_eq!(fallback.url, value);
            assert_eq!(fallback.message(), format!("{prefix}{value}"));
        }
        assert!(
            route_link(Some("https://ip.net.coffee"), External)
                .unwrap()
                .copy_fallback(OpenFailure::BrowserDidNotOpen, English)
                .is_none()
        );
    }

    #[test]
    fn sign_in_checks_both_providers_and_their_distinct_loopback_callbacks() {
        let openai = "https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback";
        let google = "https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=http%3A%2F%2F%5B%3A%3A1%5D%3A51121%2Foauth-callback";
        for value in [openai, google] {
            assert_eq!(route_link(Some(value), External), Err(UntrustedDestination));
            let link = route_link(Some(value), SignIn).unwrap();
            assert_eq!(link.url(), value);
            assert!(
                link.copy_fallback(OpenFailure::BrowserDidNotOpen, English)
                    .is_none()
            );
        }
        for value in [
            openai.replace("1455", "51121"),
            openai.replace("127.0.0.1", "127.0.0.1.invalid"),
            openai.replace("callback", "callback%3Fextra"),
            openai.replace("127.0.0.1", "user%40localhost"),
            format!("{openai}#fragment"),
            google.replace("oauth-callback", "auth%2Fcallback"),
        ] {
            assert_eq!(
                route_link(Some(&value), SignIn),
                Err(UntrustedDestination),
                "{value}"
            );
        }
    }

    #[test]
    fn opening_the_system_browser_requires_a_selected_prompt_action() {
        use ConfirmationAction::*;
        use ControlledBrowserFailure::*;

        let missing = system_browser_confirmation(Missing, Chinese);
        assert_eq!(missing.title, "Mantle 未安装");
        assert_eq!(
            missing.message,
            "从 Matou 打开的网页默认使用 Mantle 浏览器。"
        );
        assert_eq!(
            missing.detail,
            Some("安装 Mantle 后，登录和网页都会在受控环境里打开。")
        );
        assert_eq!((missing.default_index, missing.cancel_index), (0, 2));
        assert_eq!(
            missing
                .buttons
                .iter()
                .map(|button| button.label)
                .collect::<Vec<_>>(),
            ["安装 Mantle", "用系统浏览器打开", "取消"]
        );
        assert_eq!(missing.selected_action(None), Cancel);
        assert_eq!(missing.selected_action(Some(0)), InstallBrowser);
        assert_eq!(missing.selected_action(Some(1)), OpenSystemBrowser);
        assert_eq!(missing.selected_action(Some(2)), Cancel);

        let failed = system_browser_confirmation(LaunchFailed, English);
        assert_eq!(failed.title, "Mantle could not open");
        assert_eq!(failed.message, "Mantle could not open this page.");
        assert_eq!(failed.detail, None);
        assert_eq!((failed.default_index, failed.cancel_index), (0, 0));
        assert_eq!(
            failed
                .buttons
                .iter()
                .map(|button| button.label)
                .collect::<Vec<_>>(),
            ["Open in system browser", "Cancel"]
        );
        assert_eq!(failed.selected_action(None), Cancel);
        assert_eq!(failed.selected_action(Some(0)), OpenSystemBrowser);
        assert_eq!(failed.selected_action(Some(1)), Cancel);
        assert_eq!(failed.selected_action(Some(10)), Cancel);
    }
}
