//! Resolve the application's display language without changing SSH or shell locales.
pub const PREFERENCES: [&str; 3] = ["system", "zh", "en"];

pub fn from_locale(locale: &str) -> &'static str {
    let language = locale
        .trim()
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or("");
    if language.eq_ignore_ascii_case("zh") {
        "zh"
    } else {
        "en"
    }
}

pub fn resolve(preference: &str, system: &str) -> &'static str {
    match preference {
        "zh" => "zh",
        "en" => "en",
        _ => from_locale(system),
    }
}

#[cfg(any(test, not(any(windows, target_os = "macos"))))]
fn from_environment(mut lookup: impl FnMut(&str) -> Option<String>) -> &'static str {
    // POSIX message locale precedence, including an explicit C/POSIX override.
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(locale) = lookup(name).filter(|value| !value.trim().is_empty()) {
            return from_locale(&locale);
        }
    }
    "en"
}

pub fn system_language() -> &'static str {
    #[cfg(windows)]
    {
        // The user's Windows display language, rather than a regional date/number format.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        let language = unsafe { GetUserDefaultUILanguage() };
        if language & 0x03ff == 0x0004 {
            "zh"
        } else {
            "en"
        }
    }
    #[cfg(target_os = "macos")]
    {
        // AppleLanguages is the user's ordered display-language preference list.
        let preferred = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok());
        preferred
            .as_deref()
            .and_then(|output| {
                output
                    .lines()
                    .map(|line| line.trim().trim_matches(['(', ')', ',', '"', ' ']))
                    .find(|line| !line.is_empty())
            })
            .map(from_locale)
            .unwrap_or("en")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        from_environment(|name| std::env::var(name).ok())
    }
}

pub fn text<'a>(language: &str, chinese: &'a str, english: &'a str) -> &'a str {
    if language == "zh" {
        chinese
    } else {
        english
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_variants_and_only_chinese_default_to_chinese() {
        for locale in ["zh", "zh-CN", "zh-TW", "zh_HK.UTF-8", "ZH_sg", "zh_Hans_CN"] {
            assert_eq!(from_locale(locale), "zh", "{locale}");
        }
        for locale in [
            "",
            "C",
            "C.UTF-8",
            "POSIX",
            "en_US.UTF-8",
            "ja_JP",
            "fr-FR",
            "zhang",
        ] {
            assert_eq!(from_locale(locale), "en", "{locale}");
        }
    }

    #[test]
    fn posix_message_locale_precedence_and_empty_values() {
        let detect = |all: &str, messages: &str, lang: &str| {
            from_environment(|name| {
                Some(
                    match name {
                        "LC_ALL" => all,
                        "LC_MESSAGES" => messages,
                        "LANG" => lang,
                        _ => "",
                    }
                    .into(),
                )
            })
        };
        assert_eq!(detect("en_US.UTF-8", "zh_CN", "zh_CN"), "en");
        assert_eq!(detect("", "zh_TW", "en_US"), "zh");
        assert_eq!(detect(" ", "", "zh_CN.UTF-8"), "zh");
        assert_eq!(detect("C", "zh_CN", "zh_CN"), "en");
        assert_eq!(detect("", "POSIX", "zh_CN"), "en");
        assert_eq!(detect("", "", ""), "en");
    }

    #[test]
    fn explicit_preference_overrides_the_system_language() {
        assert_eq!(resolve("en", "zh-CN"), "en");
        assert_eq!(resolve("zh", "de-DE"), "zh");
        assert_eq!(resolve("system", "zh-TW"), "zh");
        assert_eq!(resolve("system", "ja-JP"), "en");
    }
}
