//! OpenBSD: direct `/etc/resolv.conf` takeover.
//!
//! OpenBSD has no resolver manager to hand `.ray` to — the system reads
//! `/etc/resolv.conf` (`resolvd` on 7.x may rewrite it on link changes, which
//! the same trample-recovery story as DHCP on Linux covers). So this is the
//! one rung of the ladder, mirroring what Linux's `DirectResolvConf` does:
//! capture the live upstreams and search domains, prove the upstreams answer,
//! back the file up, write ours, and restore on the way out.

use super::*;

/// The backup left beside `/etc/resolv.conf` while we own it.
pub(super) fn backup_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/resolv.conf.before-rayfish")
}

/// The nameservers a rendered resolv.conf names, in order, deduplicated.
fn parse_nameservers(contents: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in contents.lines() {
        if let Some(ip) = line
            .trim()
            .strip_prefix("nameserver ")
            .and_then(|ip| ip.trim().parse::<Ipv4Addr>().ok())
        {
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

pub(super) struct OpenBsdResolvConf {
    /// Upstreams that answered when we captured the file; written back as the
    /// fallback nameservers after ours. Empty means the takeover was forced by
    /// operator-configured `dns_upstreams` and nothing was captured.
    captured_upstreams: Vec<Ipv4Addr>,
    search: Vec<SearchDomain>,
    operator_upstreams: bool,
}

impl OpenBsdResolvConf {
    pub(super) async fn new() -> Self {
        let contents = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        let search: Vec<SearchDomain> = contents
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("search ")
                    .or_else(|| l.trim().strip_prefix("domain "))
            })
            .flat_map(|s| s.split_whitespace().map(SearchDomain::from_host))
            // Ours are re-derived from the joined networks on every refresh;
            // keeping the ones this file already names would outlive a leave.
            .filter(|d| !d.is_ours())
            .collect();

        let captured = parse_nameservers(&contents);
        let live = crate::dns::resolver::live_upstreams(&captured).await;
        if live.len() != captured.len() {
            let dead: Vec<_> = captured.iter().filter(|ip| !live.contains(ip)).collect();
            tracing::warn!(
                ?dead,
                "resolv.conf names DNS servers that do not answer; ignoring them"
            );
        }
        Self {
            captured_upstreams: live,
            search,
            operator_upstreams: crate::config::load()
                .map(|c| crate::config::has_usable_upstream(&c.dns_upstreams))
                .unwrap_or(false),
        }
    }

    /// The file we render: ours first, then whatever fallbacks we captured, so
    /// the host keeps resolving if our resolver stops answering.
    fn render(&self, search: &[SearchDomain]) -> String {
        let mut out = String::from("# Added by rayfish - do not edit\n");
        out.push_str(&format!("nameserver {}\n", super::resolver_addr()));
        for ip in &self.captured_upstreams {
            out.push_str(&format!("nameserver {ip}\n"));
        }
        if !search.is_empty() {
            out.push_str(&format!("search {}\n", join_domains(search)));
        }
        out
    }

    fn install(&self, search: &[SearchDomain]) -> Result<()> {
        let path = std::path::Path::new("/etc/resolv.conf");
        let backup = backup_path();
        // Capture the operator's file exactly once, before the first write: a
        // re-apply (join/leave rewriting the search list) must not back up our
        // own render over it.
        if path.exists() && !backup.exists() {
            std::fs::copy(path, &backup)
                .with_context(|| format!("backing up {}", path.display()))?;
        }
        crate::config::write_file(path, self.render(search).as_bytes(), false)
            .with_context(|| format!("writing {}", path.display()))
    }
}

#[async_trait]
impl DnsConfigurator for OpenBsdResolvConf {
    async fn apply(&self) -> Result<()> {
        // Refuse the takeover rather than install a black hole: with no
        // upstream that answers, owning resolv.conf breaks all non-`.ray`
        // resolution, and a host with working DNS and no Magic DNS is the
        // better failure.
        anyhow::ensure!(
            !self.captured_upstreams.is_empty() || self.operator_upstreams,
            "no working DNS server found in /etc/resolv.conf, so taking it over would leave \
             this host unable to resolve anything; set `dns_upstreams` in the config to \
             name one explicitly"
        );
        self.install(&self.search)
    }

    async fn revert(&self) -> Result<()> {
        let backup = backup_path();
        if backup.exists() {
            std::fs::copy(&backup, "/etc/resolv.conf")
                .with_context("restoring /etc/resolv.conf backup")?;
            std::fs::remove_file(&backup).context("removing DNS backup")?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "openbsd-resolv.conf"
    }

    fn captured_upstreams(&self) -> Vec<Ipv4Addr> {
        self.captured_upstreams.clone()
    }

    fn fallback_upstreams(&self) -> Vec<Ipv4Addr> {
        self.captured_upstreams.clone()
    }

    async fn set_search_domains(&self, domains: &[SearchDomain], _tun_name: &str) -> Result<()> {
        self.install(domains)
    }
}
