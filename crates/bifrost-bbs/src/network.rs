//! Multi-BBS Network Registry and Relay Transport Manager.
//!
//! Provides discovery, catalog sync, and relay traversal across the global
//! Bifrost Multi-BBS network.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

fn default_true() -> bool {
    true
}

fn default_max_hops() -> u8 {
    3
}

fn default_max_relays() -> u8 {
    4
}

fn default_registry_url() -> String {
    "https://raw.githubusercontent.com/bifrost-bbs/bbs-network-registry/main/registry.json"
        .to_string()
}

fn default_registry_cache_file() -> String {
    ".client_cache/network_registry.json".to_string()
}

/// Multi-BBS Network Configuration from `config.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct NetworkConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_hops")]
    pub max_hops: u8,
    #[serde(default = "default_true")]
    pub allow_inbound_relay: bool,
    #[serde(default = "default_registry_url")]
    pub registry_url: String,
    #[serde(default = "default_registry_cache_file")]
    pub registry_cache_file: String,
}

pub fn default_network_config() -> NetworkConfig {
    NetworkConfig {
        enabled: true,
        max_hops: 3,
        allow_inbound_relay: true,
        registry_url: default_registry_url(),
        registry_cache_file: default_registry_cache_file(),
    }
}

/// Schema representing the central BBS network registry index.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsRegistry {
    pub version: u32,
    pub updated_at: String,
    pub nodes: Vec<BbsNodeEntry>,
}

impl Default for BbsRegistry {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: "2026-08-25T00:00:00Z".to_string(),
            nodes: vec![
                BbsNodeEntry {
                    node_id: "0101010101010101010101010101010101010101010101010101010101010101".to_string(),
                    name: "Pacific Mesh Core Prime".to_string(),
                    callsign: "ZL1BBS".to_string(),
                    description: "Primary Auckland MeshCore relay and gateway node for the Pacific network.".to_string(),
                    location: BbsLocation {
                        lat: -36.8485,
                        lon: 174.7633,
                        grid: "RF73hd".to_string(),
                        region: "Oceania / New Zealand".to_string(),
                    },
                    endpoints: vec![
                        BbsEndpoint {
                            protocol: "tcp".to_string(),
                            host: "127.0.0.1".to_string(),
                            port: 8088,
                        },
                        BbsEndpoint {
                            protocol: "tls".to_string(),
                            host: "akl.pacificmesh.org".to_string(),
                            port: 8088,
                        },
                    ],
                    capabilities: BbsCapabilities {
                        relay_enabled: true,
                        max_inbound_relays: 8,
                        supported_apps: vec!["messages".to_string(), "profile".to_string(), "admin".to_string(), "marketplace".to_string(), "minidungeon".to_string(), "weather".to_string()],
                    },
                    sysop: BbsSysop {
                        handle: "GatewayOp".to_string(),
                        contact: "mesh:ZL1BBS".to_string(),
                    },
                    signature: Some("3045022100a1b2c3d4e5f60102030405060708090a0b0c0d0e0f101112131415161718191a".to_string()),
                },
                BbsNodeEntry {
                    node_id: "0202020202020202020202020202020202020202020202020202020202020202".to_string(),
                    name: "Wellington Capital Mesh".to_string(),
                    callsign: "ZL2BBS".to_string(),
                    description: "Government & emergency comms backup hub in Wellington, NZ.".to_string(),
                    location: BbsLocation {
                        lat: -41.2865,
                        lon: 174.7762,
                        grid: "RE78jr".to_string(),
                        region: "Oceania / New Zealand".to_string(),
                    },
                    endpoints: vec![
                        BbsEndpoint {
                            protocol: "tcp".to_string(),
                            host: "wlg.pacificmesh.org".to_string(),
                            port: 8088,
                        },
                    ],
                    capabilities: BbsCapabilities {
                        relay_enabled: true,
                        max_inbound_relays: 4,
                        supported_apps: vec!["messages".to_string(), "profile".to_string(), "weather".to_string()],
                    },
                    sysop: BbsSysop {
                        handle: "WindyCitySysop".to_string(),
                        contact: "mesh:ZL2BBS".to_string(),
                    },
                    signature: Some("3045022100b2c3d4e5f60102030405060708090a0b0c0d0e0f101112131415161718191a1b".to_string()),
                },
                BbsNodeEntry {
                    node_id: "0303030303030303030303030303030303030303030303030303030303030303".to_string(),
                    name: "Bay Area Mesh Core".to_string(),
                    callsign: "K6BBS".to_string(),
                    description: "San Francisco Bay Area emergency packet mesh interconnect.".to_string(),
                    location: BbsLocation {
                        lat: 37.7749,
                        lon: -122.4194,
                        grid: "CM87ws".to_string(),
                        region: "North America / USA".to_string(),
                    },
                    endpoints: vec![
                        BbsEndpoint {
                            protocol: "tls".to_string(),
                            host: "sf.baymesh.net".to_string(),
                            port: 8088,
                        },
                    ],
                    capabilities: BbsCapabilities {
                        relay_enabled: true,
                        max_inbound_relays: 12,
                        supported_apps: vec!["messages".to_string(), "profile".to_string(), "marketplace".to_string(), "voidtrader".to_string()],
                    },
                    sysop: BbsSysop {
                        handle: "SiliconSysop".to_string(),
                        contact: "mesh:K6BBS".to_string(),
                    },
                    signature: Some("3045022100c3d4e5f60102030405060708090a0b0c0d0e0f101112131415161718191a1b1c".to_string()),
                },
            ],
        }
    }
}

/// Metadata entry for a single participating BBS node.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsNodeEntry {
    pub node_id: String,
    pub name: String,
    pub callsign: String,
    pub description: String,
    pub location: BbsLocation,
    pub endpoints: Vec<BbsEndpoint>,
    pub capabilities: BbsCapabilities,
    pub sysop: BbsSysop,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsLocation {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub grid: String,
    pub region: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsEndpoint {
    pub protocol: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsCapabilities {
    pub relay_enabled: bool,
    #[serde(default = "default_max_relays")]
    pub max_inbound_relays: u8,
    #[serde(default)]
    pub supported_apps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BbsSysop {
    pub handle: String,
    #[serde(default)]
    pub contact: String,
}

/// Thread-safe registry manager for caching, querying, and searching BBS nodes.
pub struct BbsNetworkRegistryManager {
    registry: RwLock<BbsRegistry>,
    cache_path: PathBuf,
}

impl BbsNetworkRegistryManager {
    pub fn new(cache_path: impl AsRef<Path>) -> Self {
        let path = cache_path.as_ref().to_path_buf();
        let loaded = Self::load_from_cache_file(&path).unwrap_or_else(|e| {
            log::debug!(
                "Failed to load registry cache from {:?}: {}. Using default embedded seed.",
                path,
                e
            );
            BbsRegistry::default()
        });

        Self {
            registry: RwLock::new(loaded),
            cache_path: path,
        }
    }

    pub fn load_from_json(json_str: &str) -> Result<BbsRegistry> {
        let registry: BbsRegistry =
            serde_json::from_str(json_str).context("Failed to deserialize BbsRegistry JSON")?;
        Ok(registry)
    }

    pub fn load_from_cache_file(path: &Path) -> Result<BbsRegistry> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            Self::load_from_json(&content)
        } else {
            anyhow::bail!("Cache file does not exist: {:?}", path);
        }
    }

    pub fn save_to_cache(&self) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            if !parent.exists() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let reg = self.registry.read().unwrap();
        let json_str = serde_json::to_string_pretty(&*reg)?;
        std::fs::write(&self.cache_path, json_str)?;
        Ok(())
    }

    pub fn sync_from_url(&self, url: &str) -> Result<usize> {
        log::info!("Fetching Multi-BBS registry from {}", url);
        let resp = reqwest::blocking::get(url)
            .with_context(|| format!("Failed to fetch registry from {}", url))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "HTTP status {} fetching registry from {}",
                resp.status(),
                url
            );
        }

        let json_str = resp.text()?;
        let reg = Self::load_from_json(&json_str)?;
        let count = reg.nodes.len();

        {
            let mut guard = self.registry.write().unwrap();
            *guard = reg;
        }

        let _ = self.save_to_cache();
        log::info!(
            "Successfully synced {} BBS nodes into network registry",
            count
        );
        Ok(count)
    }

    pub fn get_nodes(&self) -> Vec<BbsNodeEntry> {
        self.registry.read().unwrap().nodes.clone()
    }

    pub fn find_by_id(&self, node_id_hex: &str) -> Option<BbsNodeEntry> {
        let guard = self.registry.read().unwrap();
        guard
            .nodes
            .iter()
            .find(|n| n.node_id.eq_ignore_ascii_case(node_id_hex))
            .cloned()
    }

    pub fn search(&self, query: &str) -> Vec<BbsNodeEntry> {
        let q = query.trim().to_lowercase();
        let guard = self.registry.read().unwrap();
        if q.is_empty() {
            return guard.nodes.clone();
        }

        guard
            .nodes
            .iter()
            .filter(|n| {
                n.name.to_lowercase().contains(&q)
                    || n.callsign.to_lowercase().contains(&q)
                    || n.description.to_lowercase().contains(&q)
                    || n.location.region.to_lowercase().contains(&q)
                    || n.location.grid.to_lowercase().contains(&q)
                    || n.node_id.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    pub fn paginate(
        nodes: &[BbsNodeEntry],
        page: usize,
        page_size: usize,
    ) -> (Vec<BbsNodeEntry>, usize, usize) {
        let page_size = page_size.max(1);
        let total_nodes = nodes.len();
        let total_pages = if total_nodes == 0 {
            1
        } else {
            (total_nodes + page_size - 1) / page_size
        };
        let current_page = page.max(1).min(total_pages);
        let start = (current_page - 1) * page_size;
        let end = (start + page_size).min(total_nodes);

        let slice = if start < total_nodes {
            nodes[start..end].to_vec()
        } else {
            Vec::new()
        };

        (slice, current_page, total_pages)
    }
}

/// Embedded Lua code for the built-in Multi-BBS Hub navigation application.
pub const EMBEDDED_NETWORK_HUB_LUA: &str = r#"
local hub = {}

local current_search_query = ""
local current_page = 1
local PAGE_SIZE = 3

function hub.render_view(session)
    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0) -- Bright Cyan
    term.print("=== BIFROST MULTI-BBS NETWORK HUB ===\n\n")
    term.set_color(7, 0)

    local nodes = nil
    if type(session.get_network_nodes) == "function" then
        nodes = session.get_network_nodes(current_search_query)
    end
    if not nodes then
        nodes = {}
    end

    local total_count = #nodes
    local total_pages = math.max(1, math.ceil(total_count / PAGE_SIZE))
    current_page = math.max(1, math.min(total_pages, current_page))

    local start_idx = (current_page - 1) * PAGE_SIZE + 1
    local end_idx = math.min(total_count, start_idx + PAGE_SIZE - 1)

    term.set_color(14, 0)
    term.print(string.format("Directory: %d BBS Nodes Active | Page %d of %d\n", total_count, current_page, total_pages))
    term.set_color(7, 0)
    term.print("----------------------------------------------------------------------\n\n")

    term.define_form(50)

    -- Render Search Input
    term.print("Search: ")
    term.add_input_field("query", 10, 6, 20, current_search_query)
    term.add_submit_button("search", 32, 6)
    term.print("\n\n")

    local y = 9
    if total_count == 0 then
        term.print("  No BBS nodes found matching your search query.\n\n")
        y = y + 2
    else
        for i = start_idx, end_idx do
            local node = nodes[i]
            term.set_color(10, 0) -- Green
            term.print(string.format(" %d. [%s] %s\n", i, node.callsign or "BBS", node.name or "Unknown"))
            term.set_color(7, 0)
            term.print(string.format("    Region: %s | Contact: %s\n", node.region or "Global", node.contact or "N/A"))
            if node.description and node.description ~= "" then
                term.set_color(8, 0)
                term.print(string.format("    \"%s\"\n", node.description))
                term.set_color(7, 0)
            end

            local btn_id = "connect_" .. tostring(i)
            term.add_submit_button(btn_id, 2, y + 3)
            term.print(string.format("    [ Relay Connect to %s ]\n\n", node.callsign or "Node"))
            y = y + 5
        end
    end

    -- Pagination and Navigation buttons
    term.print("----------------------------------------------------------------------\n")
    local nav_y = y + 1

    if current_page > 1 then
        term.add_submit_button("prev_page", 2, nav_y)
        term.print("  [ < Previous Page ]")
    end

    if current_page < total_pages then
        term.add_submit_button("next_page", 24, nav_y)
        term.print("  [ Next Page > ]")
    end

    term.add_submit_button("main_menu", 46, nav_y)
    term.print("  [ Return to Main Menu ]\n")

    term.flush_form()

    session.await_input(50, function(submission)
        if type(submission) == "string" then
            local s = submission:lower()
            if s == "m" or s == "q" or s == "b" or s == "back" or s == "exit" or s == "quit" then
                session.load_app("main_menu")
            else
                hub.render_view(session)
            end
            return
        end

        local action = submission.submit
        if action == "main_menu" or action == "back" or action == "exit" or action == "quit" then
            session.load_app("main_menu")
        elseif action == "search" then
            current_search_query = submission.query or ""
            current_page = 1
            hub.render_view(session)
        elseif action == "prev_page" then
            current_page = math.max(1, current_page - 1)
            hub.render_view(session)
        elseif action == "next_page" then
            current_page = current_page + 1
            hub.render_view(session)
        elseif action and action:sub(1, 8) == "connect_" then
            local idx = tonumber(action:sub(9))
            if idx and nodes[idx] then
                local target = nodes[idx]
                hub.connect_relay(session, target)
            else
                hub.render_view(session)
            end
        else
            hub.render_view(session)
        end
    end)
end

function hub.connect_relay(session, target_node)
    term.clear()
    term.move_to(2, 2)
    term.set_color(14, 0)
    term.print(string.format("=== CONNECTING TO %s (%s) ===\n\n", target_node.name, target_node.callsign))
    term.set_color(7, 0)
    term.print(string.format("Target Node ID: %s\n", target_node.node_id or "Unknown"))
    term.print(string.format("Location: %s\n", target_node.region or "Unknown"))
    term.print("Initiating authenticated multi-hop relay session...\n\n")

    local ok = false
    if type(session.start_relay_session) == "function" then
        ok = session.start_relay_session(target_node.node_id)
    end

    if not ok then
        term.set_color(12, 0) -- Red
        term.print("Relay Connection Failed or Node Unreachable.\n")
        term.set_color(7, 0)
        term.print("Press button to return to Network Hub.\n\n")

        term.define_form(51)
        term.add_submit_button("back_to_hub", 2, 10)
        term.print("    [ Back to Network Hub ]\n")
        term.flush_form()

        session.await_input(51, function()
            hub.render_view(session)
        end)
    end
end

function hub.on_start(session)
    log.info("Multi-BBS Network Hub application loaded.")
    hub.render_view(session)
end

function hub.on_resume(session)
    hub.render_view(session)
end

return hub
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbs_registry_defaults_and_search() {
        let reg_mgr =
            BbsNetworkRegistryManager::new(PathBuf::from(".client_cache/test_net_reg.json"));
        let all_nodes = reg_mgr.get_nodes();
        assert_eq!(all_nodes.len(), 3);

        let zl_nodes = reg_mgr.search("ZL");
        assert_eq!(zl_nodes.len(), 2);

        let bay_nodes = reg_mgr.search("Bay Area");
        assert_eq!(bay_nodes.len(), 1);
        assert_eq!(bay_nodes[0].callsign, "K6BBS");

        let empty = reg_mgr.search("NonExistentCity");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_bbs_registry_pagination() {
        let reg_mgr =
            BbsNetworkRegistryManager::new(PathBuf::from(".client_cache/test_net_reg2.json"));
        let nodes = reg_mgr.get_nodes();

        let (page1, cur, total) = BbsNetworkRegistryManager::paginate(&nodes, 1, 2);
        assert_eq!(cur, 1);
        assert_eq!(total, 2);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].callsign, "ZL1BBS");

        let (page2, cur, total) = BbsNetworkRegistryManager::paginate(&nodes, 2, 2);
        assert_eq!(cur, 2);
        assert_eq!(total, 2);
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].callsign, "K6BBS");
    }

    #[test]
    fn test_bbs_registry_json_serialization_roundtrip() {
        let reg = BbsRegistry::default();
        let json_str = serde_json::to_string(&reg).unwrap();
        let decoded = BbsNetworkRegistryManager::load_from_json(&json_str).unwrap();

        assert_eq!(decoded.version, reg.version);
        assert_eq!(decoded.nodes.len(), reg.nodes.len());
        assert_eq!(decoded.nodes[0].node_id, reg.nodes[0].node_id);
    }
}
