//! 名字和标签的写法。

use std::collections::BTreeSet;

use crate::access::ALL;
use crate::error::Error;

const MAX_LABEL_LEN: usize = 63;
const MAX_TAG_LEN: usize = 32;
const MAX_TITLE_LEN: usize = 64;

/// 网络和节点的名字：会出现在 `<节点>.<网络>.i3t` 里，所以按域名的一段来要求。返回小写的形式。
///
/// 不能全是数字：命令行里节点既可以用编号也可以用名字指代，全是数字的名字分不清是哪一种。
pub(super) fn label(what: &str, raw: &str) -> Result<String, Error> {
    let name = raw.trim().to_ascii_lowercase();
    let well_formed = !name.is_empty()
        && name.len() <= MAX_LABEL_LEN
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if !well_formed {
        return Err(Error::invalid(format!(
            "{what}的名字 {raw:?} 不合法：只能用字母、数字和连字符，不能以连字符开头或结尾，最长 {MAX_LABEL_LEN} 个字符"
        )));
    }
    if name.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::invalid(format!("{what}的名字 {raw:?} 不合法：不能全是数字")));
    }
    Ok(name)
}

/// 一个标签的写法，包括内置的 `all`。返回小写的形式。
pub(super) fn tag(raw: &str) -> Result<String, Error> {
    let tag = raw.trim().to_ascii_lowercase();
    let well_formed = !tag.is_empty()
        && tag.len() <= MAX_TAG_LEN
        && tag.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_');
    if well_formed {
        Ok(tag)
    } else {
        Err(Error::invalid(format!("标签 {raw:?} 不合法：只能用字母、数字、连字符和下划线，最长 {MAX_TAG_LEN} 个字符")))
    }
}

/// 打在节点上的一组标签。`all` 每个节点本来就有，写了也不存。
pub(super) fn node_tags(raw: &[String]) -> Result<BTreeSet<String>, Error> {
    let tags = raw.iter().map(|raw| tag(raw)).collect::<Result<BTreeSet<_>, _>>()?;
    Ok(tags.into_iter().filter(|tag| tag != ALL).collect())
}

/// 规则、凭据、监听器、端口转发的名字：给人看的一小段文字。`required` 为真时不能是空的。
pub(super) fn title(what: &str, raw: &str, required: bool) -> Result<String, Error> {
    let title = raw.trim();
    if required && title.is_empty() {
        return Err(Error::invalid(format!("{what}的名字不能是空的")));
    }
    if title.chars().count() > MAX_TITLE_LEN || title.chars().any(char::is_control) {
        return Err(Error::invalid(format!("{what}的名字不合法：最长 {MAX_TITLE_LEN} 个字符，不能有控制字符")));
    }
    Ok(title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_lowercased_dns_labels() {
        assert_eq!(label("节点", " NAS-01 ").unwrap(), "nas-01");
        for bad in ["", "-a", "a-", "a.b", "a b", "名字", "123", &"a".repeat(64)] {
            assert!(label("节点", bad).is_err(), "{bad}");
        }
        assert!(label("节点", &"a".repeat(63)).is_ok());
        assert!(label("节点", "7zip").is_ok());
    }

    #[test]
    fn node_tags_are_normalised_and_the_builtin_tag_is_not_stored() {
        let tags = node_tags(&["Servers".to_owned(), "all".to_owned(), "home_lab".to_owned(), "servers".to_owned()]).unwrap();
        assert_eq!(tags.into_iter().collect::<Vec<_>>(), vec!["home_lab".to_owned(), "servers".to_owned()]);
        assert!(node_tags(&["a b".to_owned()]).is_err());
        assert!(node_tags(&[String::new()]).is_err());
        assert_eq!(tag("ALL").unwrap(), ALL);
    }

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(title("规则", "  laptops → servers ", false).unwrap(), "laptops → servers");
        assert_eq!(title("规则", "", false).unwrap(), "");
        assert!(title("凭据", "  ", true).is_err());
        assert!(title("规则", "a\nb", false).is_err());
        assert!(title("规则", &"规".repeat(65), false).is_err());
    }
}
