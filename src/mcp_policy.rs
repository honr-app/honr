//! Merge MCP server policy fragments into a sandbox OpenShell policy YAML.

use serde_yaml::Value;

/// Merge `network_policies` (and optional top-level keys) from fragments into
/// `base_yaml`. Fragment may be a full policy document or a bare mapping that
/// is treated as `network_policies` entries.
pub fn merge_policy_fragments(
    base_yaml: &str,
    fragments: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<String, String> {
    let frags: Vec<String> = fragments
        .into_iter()
        .map(|f| f.as_ref().trim().to_string())
        .filter(|f| !f.is_empty())
        .collect();
    if frags.is_empty() {
        return Ok(base_yaml.to_string());
    }

    let mut base: Value = if base_yaml.trim().is_empty() {
        Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(base_yaml).map_err(|e| format!("base policy yaml: {e}"))?
    };
    if !base.is_mapping() {
        return Err("base policy yaml must be a mapping".into());
    }

    for raw in &frags {
        let parsed: Value =
            serde_yaml::from_str(raw).map_err(|e| format!("mcp policy fragment: {e}"))?;
        let net = extract_network_policies(&parsed)?;
        merge_network_policies(&mut base, net)?;
    }

    serde_yaml::to_string(&base).map_err(|e| format!("serialize merged policy: {e}"))
}

fn extract_network_policies(parsed: &Value) -> Result<serde_yaml::Mapping, String> {
    let Some(map) = parsed.as_mapping() else {
        return Err("mcp policy fragment must be a YAML mapping".into());
    };
    if let Some(Value::Mapping(net)) = map.get(Value::String("network_policies".into())) {
        return Ok(net.clone());
    }
    // Bare network_policies body (keys are policy names).
    if map
        .keys()
        .all(|k| k.as_str().is_some_and(|s| s != "version" && s != "filesystem_policy"))
        && !map.is_empty()
    {
        return Ok(map.clone());
    }
    Ok(serde_yaml::Mapping::new())
}

fn merge_network_policies(base: &mut Value, incoming: serde_yaml::Mapping) -> Result<(), String> {
    let Value::Mapping(root) = base else {
        return Err("base policy yaml must be a mapping".into());
    };
    let key = Value::String("network_policies".into());
    if !root.contains_key(&key) {
        root.insert(key.clone(), Value::Mapping(serde_yaml::Mapping::new()));
    }
    let Some(Value::Mapping(net)) = root.get_mut(&key) else {
        return Err("network_policies must be a mapping".into());
    };
    for (k, v) in incoming {
        net.insert(k, v);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_named_network_policy_from_fragment() {
        let base = r#"
version: 1
network_policies:
  existing:
    name: existing
    endpoints: []
"#;
        let frag = r#"
network_policies:
  pypi:
    name: pypi
    endpoints:
      - { host: pypi.org, port: 443, access: full, tls: skip }
"#;
        let out = merge_policy_fragments(base, [frag]).expect("merge");
        assert!(out.contains("existing:"));
        assert!(out.contains("pypi:"));
        assert!(out.contains("pypi.org"));
    }

    #[test]
    fn accepts_bare_network_policies_map() {
        let base = "version: 1\nnetwork_policies: {}\n";
        let frag = "hf:\n  name: hf\n  endpoints: [{ host: huggingface.co, port: 443, access: full, tls: skip }]\n";
        let out = merge_policy_fragments(base, [frag]).expect("merge");
        assert!(out.contains("huggingface.co"));
        assert!(out.contains("tls: skip") || out.contains("tls:skip"));
    }
}
