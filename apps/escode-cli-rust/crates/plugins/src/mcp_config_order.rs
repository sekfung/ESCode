//! MCP server 合并的键序（TS `{ ...pluginMcpServers, ...configured, ...builtInMcpServers }`）。
//!
//! JS 对象 spread 的键序语义：新键追加到末尾，已有键原位覆盖值（位置不变）。模型请求里的 MCP 工具
//! 顺序由这里的 server 顺序决定，不能用按名字排序的 map 代替。

/// 按首次插入排序的键值表。
pub struct Ordered<V>(Vec<(String, V)>);

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<V> Ordered<V> {
    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(existing, _)| existing == key)
    }

    /// 已有键原位覆盖值，新键追加。
    pub fn insert(&mut self, key: String, value: V) {
        match self.0.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str, &V) -> bool) {
        self.0.retain(|(key, value)| keep(key, value));
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn into_values(self) -> Vec<V> {
        self.0.into_iter().map(|(_, value)| value).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_js_object_spread_key_order() {
        let mut merged = Ordered::default();
        // 插件 server 先插入，内置 node_repl 最后合并（TS builtInMcpServers 最后 spread）。
        merged.insert("plugin:usb:usb".to_owned(), 1);
        merged.insert("user".to_owned(), 2);
        merged.insert("node_repl".to_owned(), 3);
        // 同名配置覆盖值但不改位置。
        merged.insert("plugin:usb:usb".to_owned(), 4);
        assert!(merged.contains("user"));
        merged.retain(|key, _| key != "user");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.into_values(), vec![4, 3]);
    }
}
