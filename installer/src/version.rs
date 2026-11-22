//! Version eligibility shared by the UI and installation worker.

use semver::Version;
use serde::Serialize;
use std::cmp::Ordering;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateEligibility {
    pub allowed: bool,
    pub title: &'static str,
    pub message: String,
}

pub fn check(installed: Option<&str>, incoming: &str) -> UpdateEligibility {
    let parse =
        |value: &str| Version::parse(value.trim().strip_prefix('v').unwrap_or(value.trim()));
    let existing = installed.and_then(|value| parse(value).ok());
    let incoming = parse(incoming).ok();
    match (existing, incoming) {
        (Some(existing), Some(incoming)) => match incoming.cmp_precedence(&existing) {
            Ordering::Greater => UpdateEligibility {
                allowed: true,
                title: "准备更新",
                message: format!("将 Glide-Dock 从 {existing} 更新到 {incoming}，保留当前设置和 Dock 条目。"),
            },
            Ordering::Equal => UpdateEligibility {
                allowed: false,
                title: "已安装此版本",
                message: format!("当前已安装 Glide-Dock {existing}，无需重复安装。卸载请使用安装目录中的卸载程序或 Windows“已安装的应用”。"),
            },
            Ordering::Less => UpdateEligibility {
                allowed: false,
                title: "安装包版本较旧",
                message: format!("当前已安装 {existing}，此安装包为 {incoming}。请下载更高版本，不能用旧包覆盖。"),
            },
        },
        _ => UpdateEligibility {
            allowed: false,
            title: "无法判断已安装版本",
            message: "安装版本信息缺失或无法识别，不能确认此安装包是否更新。请先通过 Windows“已安装的应用”卸载，再安装。".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_only_higher_semantic_versions() {
        for (old, new, allowed) in [
            ("0.4.1", "0.4.2", true),
            ("0.4.2", "0.4.2", false),
            ("0.5.0", "0.4.2", false),
            ("0.4.9", "0.4.10", true),
            ("0.4.2-beta.2", "0.4.2-beta.11", true),
            ("0.4.2-beta.2", "0.4.2", true),
            ("0.4.2", "0.4.2-beta.2", false),
            ("0.4.2+old", "0.4.2+new", false),
            ("v0.4.1", "0.4.2", true),
        ] {
            assert_eq!(check(Some(old), new).allowed, allowed, "{old} -> {new}");
        }
    }

    #[test]
    fn rejects_unknown_versions_without_guessing() {
        for existing in [None, Some(""), Some("unknown"), Some("0.4")] {
            assert!(!check(existing, "0.4.2").allowed);
        }
        assert!(!check(Some("0.4.1"), "invalid").allowed);
    }
}
