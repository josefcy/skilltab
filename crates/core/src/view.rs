//! The only types that cross the IPC boundary, and the pure builders from `model` to them.
//! tauri-specta generates `bindings.ts` from these, so the TS side never declares a backend type
//! by hand. Model types are not `Serialize`. Wire shape changes stay in this file.
//!
//! Every label, one-line explanation and derived verdict ("unused", "problem") is decided here,
//! so the UI renders what it is sent and filters on these fields without domain rules of its own.

use crate::harness::BundleOp;
use crate::manage::{ApplyOutcome, Blocker, Effect, ManageAction, Plan, Step, StepResult};
use crate::model::*;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Usage of one (harness, invocation key) pair. Empty until the usage index plugs in.
pub(crate) type Tallies = HashMap<(HarnessId, InvocationKey), Tally>;

const SKILL_MD_LIMIT: u64 = 512 * 1024;

// ---------------------------------------------------------------- wire types

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Bumped by every refresh. The UI re-fetches when the `inventory-changed` event carries a new one.
    pub revision: u32,
    pub scan_ms: u32,
    pub rows: Vec<SkillRow>,
    pub bundles: Vec<BundleRow>,
    pub problems: Problems,
    pub facets: Facets,
    /// Earliest transcript time per harness. A harness missing here has no usage data, which is not the same as zero uses.
    pub tracking_since: Vec<TrackingView>,
    /// Links the user confirmed from an old usage key to an installed skill. The UI lists them so each can be undone.
    pub aliases: Vec<AliasView>,
    /// Mounts skilltab took away and can put back. They are the disabled rows.
    pub shelf: Vec<ShelfRow>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShelfRow {
    pub id: String,
    pub name: String,
    pub harness: Tag,
    pub entry: String,
    pub reason: ShelfReasonView,
    /// What restoring does, as a sentence.
    pub note: String,
    pub at: String,
}

#[derive(Serialize, Type, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ShelfReasonView {
    Disabled,
    Removed,
}

/// An id plus the text to show for it.
#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub harnesses: Vec<Tag>,
    pub scopes: Vec<ScopeView>,
    pub owners: Vec<Tag>,
    pub origin: OriginView,
    /// Ids of the bundles this body belongs to.
    pub bundles: Vec<String>,
    pub mounts: Vec<MountSummary>,
    pub usage: UsageSummary,
    pub flags: Vec<Flag>,
}

#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MountSummary {
    pub id: String,
    pub harness: Tag,
    pub scope: ScopeView,
    pub owner: Tag,
    pub state: MountStateView,
    pub entry: String,
}

#[derive(Serialize, Type, Clone, PartialEq, Eq, Hash, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ScopeKind {
    User,
    Project,
    Bundle,
}

#[derive(Serialize, Type, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    pub kind: ScopeKind,
    /// "user", the project folder name, or the bundle name.
    pub label: String,
    pub path: Option<String>,
    pub bundle: Option<String>,
}

#[derive(Serialize, Type, Clone)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MountStateView {
    Active,
    Shadowed { by: String },
    BundleDisabled,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OriginView {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    /// false when no harness of this skill has transcripts yet. The UI says "no data yet" instead of 0.
    pub tracked: bool,
    pub total: u32,
    pub last30d: u32,
    pub last_used: Option<String>,
    pub days_since_last_use: Option<u32>,
    /// Tracked, and no use since tracking began.
    pub unused: bool,
    pub by_harness: Vec<HarnessUsage>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HarnessUsage {
    pub harness: Tag,
    pub tracked: bool,
    pub tracking_since: Option<String>,
    pub total: u32,
    pub last30d: u32,
    pub last_used: Option<String>,
}

#[derive(Serialize, Type, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Flag {
    Shadowed,
    NameConflict,
    ManifestIssue,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BundleRow {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub harness: Tag,
    pub version: Option<String>,
    pub enabled: bool,
    pub member_count: u32,
    pub scope: ScopeView,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Problems {
    pub broken_links: Vec<BrokenLinkRow>,
    pub name_conflicts: Vec<NameConflict>,
    /// Unreadable folders and link cycles, as sentences.
    pub other: Vec<String>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BrokenLinkRow {
    pub id: String,
    pub harness: Tag,
    pub link: String,
    pub target: String,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NameConflict {
    pub name: String,
    pub identical_content: bool,
    pub members: Vec<ConflictMember>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConflictMember {
    pub skill: String,
    pub dir: String,
    pub harnesses: Vec<Tag>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Facets {
    pub harnesses: Vec<FacetValue>,
    pub scope_kinds: Vec<FacetValue>,
    pub bundles: Vec<FacetValue>,
    pub owners: Vec<FacetValue>,
    pub origins: Vec<FacetValue>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FacetValue {
    pub id: String,
    pub label: String,
    /// Rows that carry this value.
    pub count: u32,
    /// For harnesses: whether the harness has anything installed on this machine.
    pub present: bool,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrackingView {
    pub harness: Tag,
    pub since: String,
}

// ---------------------------------------------------------------- detail

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub row: SkillRow,
    pub dir: String,
    pub frontmatter: Vec<Field>,
    pub markdown: String,
    pub markdown_truncated: bool,
    pub mounts: Vec<MountDetail>,
    pub origin_note: String,
    /// One sentence per flag on the row.
    pub problem_notes: Vec<String>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: String,
    pub value: String,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MountDetail {
    pub summary: MountSummary,
    pub owner_note: String,
    /// Entry first, real folder last. A hop's index is what `Skilltab::reveal` takes.
    pub chain: Vec<HopView>,
    /// The names a harness uses for this mount in its logs and menus.
    pub keys: Vec<String>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HopView {
    pub path: String,
    pub kind: HopKind,
}

#[derive(Serialize, Type, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum HopKind {
    Link,
    Folder,
}

#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    pub revision: u32,
    pub rows: u32,
    pub scan_ms: u32,
}

#[derive(Serialize, Type, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CoreError {
    UnknownId { id: String },
    Io { message: String },
    /// The request was well formed but cannot be done, such as aliasing a key that still resolves.
    Invalid { message: String },
}

// ---------------------------------------------------------------- builders

pub(crate) fn inventory(snap: &Snapshot, tallies: &Tallies, tracking: &TrackingSince, scan_ms: u32) -> Inventory {
    let cx = Cx::new(snap, tallies, tracking);

    let mut rows: Vec<SkillRow> = snap.bodies.values().map(|b| cx.row(b)).collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));

    let mut bundles: Vec<BundleRow> = snap.bundles.values().map(|b| cx.bundle_row(b)).collect();
    bundles.sort_by(|a, b| (&a.harness.id, a.name.to_lowercase(), &a.marketplace).cmp(&(&b.harness.id, b.name.to_lowercase(), &b.marketplace)));

    let mut tracking_view: Vec<TrackingView> = snap
        .harnesses
        .iter()
        .filter_map(|h| tracking.get(&h.id).map(|t| TrackingView { harness: cx.tag(h.id), since: t.to_rfc3339() }))
        .collect();
    tracking_view.sort_by(|a, b| a.harness.id.cmp(&b.harness.id));

    let facets = cx.facets(&rows, &bundles);
    Inventory {
        revision: snap.revision as u32,
        scan_ms,
        problems: cx.problems(),
        rows,
        bundles,
        facets,
        tracking_since: tracking_view,
        aliases: Vec::new(),
        shelf: cx.shelf_rows(),
    }
}

pub(crate) fn detail(snap: &Snapshot, id: &str, tallies: &Tallies, tracking: &TrackingSince) -> Result<SkillDetail, CoreError> {
    let body = snap
        .bodies
        .values()
        .find(|b| b.id.as_str() == id)
        .ok_or_else(|| CoreError::UnknownId { id: id.to_owned() })?;
    let cx = Cx::new(snap, tallies, tracking);
    let row = cx.row(body);

    let (text, truncated) = read_skill_md(body.dir.as_path());
    let (frontmatter, markdown) = split_frontmatter(&text);

    let mounts = cx
        .mounts_of(body)
        .into_iter()
        .map(|m| MountDetail {
            summary: cx.mount_summary(m),
            owner_note: cx.owner_note(m),
            chain: chain_paths(snap, m)
                .into_iter()
                .enumerate()
                .map(|(i, p)| HopView {
                    path: path_str(&p),
                    kind: if i < m.hops.len() { HopKind::Link } else { HopKind::Folder },
                })
                .collect(),
            keys: m.keys.iter().filter_map(name_key).collect(),
        })
        .collect();

    Ok(SkillDetail {
        dir: path_str(body.dir.as_path()),
        frontmatter,
        markdown,
        markdown_truncated: truncated,
        mounts,
        origin_note: cx.origin_note(body),
        problem_notes: cx.problem_notes(body, &row),
        row,
    })
}

/// Entry, each further hop, then the real folder. For a mount whose entry is the folder itself the chain is that one path.
pub(crate) fn chain_paths(snap: &Snapshot, m: &Mount) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = m.hops.iter().map(|h| h.as_path().to_path_buf()).collect();
    match snap.bodies.get(&m.body) {
        Some(b) => out.push(b.dir.as_path().to_path_buf()),
        None => out.push(m.entry.clone()),
    }
    out
}

struct Cx<'a> {
    snap: &'a Snapshot,
    tallies: &'a Tallies,
    tracking: &'a TrackingSince,
    harness_label: HashMap<HarnessId, &'static str>,
    harness_order: HashMap<HarnessId, usize>,
    /// Bodies per manifest name, for the name-conflict flag and tab.
    by_name: HashMap<&'a str, Vec<&'a SkillBody>>,
}

impl<'a> Cx<'a> {
    fn new(snap: &'a Snapshot, tallies: &'a Tallies, tracking: &'a TrackingSince) -> Self {
        let mut by_name: HashMap<&str, Vec<&SkillBody>> = HashMap::new();
        for b in snap.bodies.values() {
            by_name.entry(b.manifest.name.as_str()).or_default().push(b);
        }
        Cx {
            snap,
            tallies,
            tracking,
            harness_label: snap.harnesses.iter().map(|h| (h.id, h.label)).collect(),
            harness_order: snap.harnesses.iter().enumerate().map(|(i, h)| (h.id, i)).collect(),
            by_name,
        }
    }

    fn tag(&self, h: HarnessId) -> Tag {
        Tag { id: h.as_str().to_owned(), label: self.harness_label.get(&h).copied().unwrap_or(h.as_str()).to_owned() }
    }

    fn mounts_of(&self, body: &SkillBody) -> Vec<&'a Mount> {
        let mut ms: Vec<&Mount> = body.mounts.iter().filter_map(|id| self.snap.mounts.get(id)).collect();
        ms.sort_by(|a, b| {
            let rank = |m: &Mount| self.harness_order.get(&m.harness).copied().unwrap_or(usize::MAX);
            rank(a).cmp(&rank(b)).then_with(|| a.entry.cmp(&b.entry))
        });
        ms
    }

    fn row(&self, body: &SkillBody) -> SkillRow {
        let mounts = self.mounts_of(body);
        let summaries: Vec<MountSummary> = mounts.iter().map(|m| self.mount_summary(m)).collect();

        let mut harnesses: Vec<Tag> = Vec::new();
        let mut owners: Vec<Tag> = Vec::new();
        let mut scopes: Vec<ScopeView> = Vec::new();
        for s in &summaries {
            if !harnesses.iter().any(|t| t.id == s.harness.id) {
                harnesses.push(s.harness.clone());
            }
            if !owners.iter().any(|t| t.id == s.owner.id) {
                owners.push(s.owner.clone());
            }
            if !scopes.contains(&s.scope) {
                scopes.push(s.scope.clone());
            }
        }

        let mut bundles: Vec<String> = scopes.iter().filter_map(|s| s.bundle.clone()).collect();
        if let Origin::Plugin { bundle } = &body.origin {
            bundles.push(bundle.as_str().to_owned());
        }
        bundles.sort();
        bundles.dedup();

        SkillRow {
            id: body.id.as_str().to_owned(),
            name: body.manifest.name.clone(),
            description: body.manifest.description.clone(),
            harnesses,
            scopes,
            owners,
            origin: self.origin(&body.origin),
            bundles,
            usage: self.usage(&mounts),
            flags: self.flags(body, &mounts),
            mounts: summaries,
        }
    }

    fn flags(&self, body: &SkillBody, mounts: &[&Mount]) -> Vec<Flag> {
        let mut f = Vec::new();
        if mounts.iter().any(|m| matches!(m.state, MountState::Shadowed { .. })) {
            f.push(Flag::Shadowed);
        }
        if self.by_name.get(body.manifest.name.as_str()).is_some_and(|v| v.len() > 1) {
            f.push(Flag::NameConflict);
        }
        if !body.manifest.issues.is_empty() {
            f.push(Flag::ManifestIssue);
        }
        f
    }

    fn mount_summary(&self, m: &Mount) -> MountSummary {
        MountSummary {
            id: m.id.as_str().to_owned(),
            harness: self.tag(m.harness),
            scope: self.scope(&m.scope),
            owner: owner_tag(&m.owner),
            state: match &m.state {
                MountState::Active => MountStateView::Active,
                MountState::Shadowed { by } => {
                    MountStateView::Shadowed { by: self.snap.mounts.get(by).map(|w| path_str(&w.entry)).unwrap_or_default() }
                }
                MountState::BundleDisabled => MountStateView::BundleDisabled,
            },
            entry: path_str(&m.entry),
        }
    }

    fn scope(&self, s: &Scope) -> ScopeView {
        match s {
            Scope::User => ScopeView { kind: ScopeKind::User, label: "user".into(), path: None, bundle: None },
            Scope::Project { root } => project_scope(root),
            Scope::Bundle { bundle } => ScopeView {
                kind: ScopeKind::Bundle,
                label: self.snap.bundles.get(bundle).map_or_else(|| bundle.as_str().to_owned(), |b| b.name.clone()),
                path: None,
                bundle: Some(bundle.as_str().to_owned()),
            },
        }
    }

    fn origin(&self, o: &Origin) -> OriginView {
        let (id, label, detail) = match o {
            Origin::JstackRepo { skill } => ("jstackRepo", "jstack repo", Some(skill.clone())),
            Origin::SkillsCli { source, .. } => ("skillsCli", "skills CLI", Some(source.clone())),
            Origin::Plugin { bundle } => ("plugin", "plugin", Some(self.bundle_name(bundle))),
            Origin::ClaudeAiSync { .. } => ("claudeAiSync", "claude.ai sync", None),
            Origin::HarnessBuiltin { harness } => ("harnessBuiltin", "built in", Some(self.tag(*harness).label)),
            Origin::Unknown => ("unknown", "unknown", None),
        };
        OriginView { id: id.into(), label: label.into(), detail }
    }

    fn bundle_name(&self, id: &BundleId) -> String {
        self.snap.bundles.get(id).map_or_else(|| id.as_str().to_owned(), |b| format!("{}@{}", b.name, b.marketplace))
    }

    fn usage(&self, mounts: &[&Mount]) -> UsageSummary {
        let mut keys: HashSet<(HarnessId, &InvocationKey)> = HashSet::new();
        let mut harnesses: Vec<HarnessId> = Vec::new();
        for m in mounts {
            if !harnesses.contains(&m.harness) {
                harnesses.push(m.harness);
            }
            keys.extend(m.keys.iter().map(|k| (m.harness, k)));
        }

        let mut by_harness = Vec::new();
        let (mut total, mut last30d) = (0u32, 0u32);
        let mut last_used: Option<Timestamp> = None;
        for h in harnesses {
            let since = self.tracking.get(&h).copied();
            let (mut t, mut l30) = (0u32, 0u32);
            let mut last: Option<Timestamp> = None;
            for (kh, key) in &keys {
                if *kh != h {
                    continue;
                }
                if let Some(tally) = self.tallies.get(&(h, (*key).clone())) {
                    t += tally.total;
                    l30 += tally.last_30d;
                    last = last.max(tally.last_used);
                }
            }
            total += t;
            last30d += l30;
            last_used = last_used.max(last);
            by_harness.push(HarnessUsage {
                harness: self.tag(h),
                tracked: since.is_some(),
                tracking_since: since.map(|s| s.to_rfc3339()),
                total: t,
                last30d: l30,
                last_used: last.map(|l| l.to_rfc3339()),
            });
        }

        let tracked = by_harness.iter().any(|h| h.tracked);
        UsageSummary {
            tracked,
            total,
            last30d,
            last_used: last_used.map(|l| l.to_rfc3339()),
            days_since_last_use: last_used.map(|l| (self.snap.taken_at - l).num_days().max(0) as u32),
            unused: tracked && total == 0,
            by_harness,
        }
    }

    fn bundle_row(&self, b: &Bundle) -> BundleRow {
        BundleRow {
            id: b.id.as_str().to_owned(),
            name: b.name.clone(),
            marketplace: b.marketplace.clone(),
            harness: self.tag(b.harness),
            version: b.version.clone(),
            enabled: b.enabled,
            member_count: b.members.len() as u32,
            scope: match &b.install_scope {
                InstallScope::User => self.scope(&Scope::User),
                InstallScope::Project { root } => project_scope(root),
            },
        }
    }

    fn problems(&self) -> Problems {
        let mut broken_links = Vec::new();
        let mut other = Vec::new();
        for i in &self.snap.issues {
            match i {
                ScanIssue::BrokenLink { id, link, target, harness } => broken_links.push(BrokenLinkRow {
                    id: id.as_str().to_owned(),
                    harness: self.tag(*harness),
                    link: path_str(link.as_path()),
                    target: path_str(target),
                }),
                ScanIssue::Unreadable { path, error } => other.push(format!("{}: {error}", path_str(path))),
                ScanIssue::LinkCycle { entry } => other.push(format!("{} links back to itself", path_str(entry))),
            }
        }

        let mut name_conflicts: Vec<NameConflict> = self
            .by_name
            .iter()
            .filter(|(_, bodies)| bodies.len() > 1)
            .map(|(name, bodies)| {
                let mut members: Vec<ConflictMember> = bodies
                    .iter()
                    .map(|b| ConflictMember {
                        skill: b.id.as_str().to_owned(),
                        dir: path_str(b.dir.as_path()),
                        harnesses: self.row_harnesses(b),
                    })
                    .collect();
                members.sort_by(|a, b| a.dir.cmp(&b.dir));
                let first = &bodies[0].content_hash;
                NameConflict {
                    name: (*name).to_owned(),
                    identical_content: bodies.iter().all(|b| &b.content_hash == first),
                    members,
                }
            })
            .collect();
        name_conflicts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        broken_links.sort_by(|a, b| a.link.cmp(&b.link));
        other.sort();

        Problems { broken_links, name_conflicts, other }
    }

    fn row_harnesses(&self, body: &SkillBody) -> Vec<Tag> {
        let mut out: Vec<Tag> = Vec::new();
        for m in self.mounts_of(body) {
            let t = self.tag(m.harness);
            if !out.iter().any(|o| o.id == t.id) {
                out.push(t);
            }
        }
        out
    }

    fn facets(&self, rows: &[SkillRow], bundles: &[BundleRow]) -> Facets {
        let harnesses = self
            .snap
            .harnesses
            .iter()
            .map(|h| FacetValue {
                id: h.id.as_str().to_owned(),
                label: h.label.to_owned(),
                count: rows.iter().filter(|r| r.harnesses.iter().any(|t| t.id == h.id.as_str())).count() as u32,
                present: h.present,
            })
            .collect();

        let scope_kinds = [(ScopeKind::User, "user", "user"), (ScopeKind::Project, "project", "project"), (ScopeKind::Bundle, "bundle", "bundle")]
            .into_iter()
            .map(|(kind, id, label)| FacetValue {
                id: id.into(),
                label: label.into(),
                count: rows.iter().filter(|r| r.scopes.iter().any(|s| s.kind == kind)).count() as u32,
                present: true,
            })
            .collect();

        let bundle_facets = bundles
            .iter()
            .map(|b| FacetValue {
                id: b.id.clone(),
                label: format!("{}@{}", b.name, b.marketplace),
                count: rows.iter().filter(|r| r.bundles.contains(&b.id)).count() as u32,
                present: true,
            })
            .collect();

        let mut owners: BTreeMap<String, FacetValue> = BTreeMap::new();
        let mut origins: BTreeMap<String, FacetValue> = BTreeMap::new();
        for r in rows {
            for o in &r.owners {
                owners.entry(o.id.clone()).or_insert_with(|| FacetValue { id: o.id.clone(), label: o.label.clone(), count: 0, present: true }).count += 1;
            }
            origins
                .entry(r.origin.id.clone())
                .or_insert_with(|| FacetValue { id: r.origin.id.clone(), label: r.origin.label.clone(), count: 0, present: true })
                .count += 1;
        }

        Facets {
            harnesses,
            scope_kinds,
            bundles: bundle_facets,
            owners: owners.into_values().collect(),
            origins: origins.into_values().collect(),
        }
    }

    fn shelf_rows(&self) -> Vec<ShelfRow> {
        let mut rows: Vec<ShelfRow> = self
            .snap
            .shelf
            .iter()
            .map(|e| ShelfRow {
                id: e.id.as_str().to_owned(),
                name: e.name.clone(),
                harness: self.tag(e.harness),
                entry: path_str(&e.entry),
                reason: match e.reason {
                    ShelfReason::Disabled => ShelfReasonView::Disabled,
                    ShelfReason::Removed => ShelfReasonView::Removed,
                },
                note: match &e.held {
                    Held::Link { target } => format!("Restoring recreates the link to {}.", path_str(target)),
                    Held::Folder { stash } => format!("The folder is kept at {} until it is restored.", path_str(stash)),
                },
                at: e.at.to_rfc3339(),
            })
            .collect();
        rows.sort_by(|a, b| b.at.cmp(&a.at));
        rows
    }

    fn owner_note(&self, m: &Mount) -> String {
        match &m.owner {
            Ownership::JstackSync => "Listed in ~/.jstack-links.txt, so jstack's sync recreates this link at the next session start.".into(),
            Ownership::SkillsCli => "Created by the skills CLI and recorded in ~/.agents/.skill-lock.json.".into(),
            Ownership::PluginManager { bundle } => {
                format!("Part of plugin {}. The harness's plugin manager owns this folder.", self.bundle_name(bundle))
            }
            Ownership::ClaudeAiSync => "Written by claude.ai sync under ~/.claude/skills/synced and rewritten on the next sync.".into(),
            Ownership::HarnessBuiltin => format!("Ships with {} and is not meant to be edited.", self.tag(m.harness).label),
            Ownership::Skilltab => "Put back by skilltab from its shelf.".into(),
            Ownership::Unmanaged => "No tool manages this entry. It was made or copied by hand.".into(),
        }
    }

    fn origin_note(&self, body: &SkillBody) -> String {
        match &body.origin {
            Origin::JstackRepo { skill } => format!("The folder sits in your jstack clone as skills/{skill}, so jstack is the source."),
            Origin::SkillsCli { source, git_ref, .. } => match git_ref {
                Some(r) => format!("Installed from {source} at {r} by the skills CLI."),
                None => format!("Installed from {source} by the skills CLI."),
            },
            Origin::Plugin { bundle } => {
                let version = self.snap.bundles.get(bundle).and_then(|b| b.version.as_deref()).map(|v| format!(" v{v}")).unwrap_or_default();
                format!("Shipped inside plugin {}{version}.", self.bundle_name(bundle))
            }
            Origin::ClaudeAiSync { .. } => "Synced down from your claude.ai account.".into(),
            Origin::HarnessBuiltin { harness } => format!("Bundled with {}.", self.tag(*harness).label),
            Origin::Unknown => "No record says where this folder came from.".into(),
        }
    }

    fn problem_notes(&self, body: &SkillBody, row: &SkillRow) -> Vec<String> {
        let mut notes = Vec::new();
        for m in self.mounts_of(body) {
            if let MountState::Shadowed { by } = &m.state {
                let winner = self.snap.mounts.get(by).map(|w| path_str(&w.entry)).unwrap_or_default();
                notes.push(format!("In {} the mount {} is shadowed by {winner}, which the harness loads instead.", self.tag(m.harness).label, path_str(&m.entry)));
            }
        }
        if row.flags.contains(&Flag::NameConflict) {
            let others = self.by_name.get(body.manifest.name.as_str()).map_or(0, |v| v.len().saturating_sub(1));
            let (noun, verb) = if others == 1 { ("skill", "shares") } else { ("skills", "share") };
            notes.push(format!("{others} other installed {noun} {verb} the name \"{}\".", body.manifest.name));
        }
        for i in &body.manifest.issues {
            notes.push(match i {
                ManifestIssue::NoFrontmatter => "SKILL.md has no frontmatter, so the folder name stands in for the name.".to_owned(),
                ManifestIssue::MissingName => "The frontmatter has no name.".to_owned(),
                ManifestIssue::NameDiffersFromFolder { folder } => format!("The frontmatter name differs from the folder name \"{folder}\"."),
                ManifestIssue::MissingDescription => "The frontmatter has no description.".to_owned(),
            });
        }
        notes
    }
}

fn project_scope(root: &ProjectRoot) -> ScopeView {
    let p = root.path();
    let label = p.file_name().map_or_else(|| path_str(p), |n| n.to_string_lossy().into_owned());
    ScopeView { kind: ScopeKind::Project, label, path: Some(path_str(p)), bundle: None }
}

fn owner_tag(o: &Ownership) -> Tag {
    let (id, label) = match o {
        Ownership::JstackSync => ("jstackSync", "jstack sync"),
        Ownership::SkillsCli => ("skillsCli", "skills CLI"),
        Ownership::PluginManager { .. } => ("pluginManager", "plugin manager"),
        Ownership::ClaudeAiSync => ("claudeAiSync", "claude.ai sync"),
        Ownership::HarnessBuiltin => ("harnessBuiltin", "built in"),
        Ownership::Skilltab => ("skilltab", "skilltab"),
        Ownership::Unmanaged => ("unmanaged", "unmanaged"),
    };
    Tag { id: id.into(), label: label.into() }
}

fn name_key(k: &InvocationKey) -> Option<String> {
    match k {
        InvocationKey::Name(n) => Some(n.clone()),
        InvocationKey::SkillFile(_) => None,
    }
}

/// Drops the `\\?\` verbatim prefix that canonicalized paths carry. Explorer and humans both prefer the plain form.
pub(crate) fn path_str(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).or_else(|| s.strip_prefix(r"\\?\").map(str::to_owned)).unwrap_or_else(|| s.into_owned())
}

// ---------------------------------------------------------------- aliases

/// A proposed link from a ghost usage key to an installed skill. Nothing is merged until the user confirms it.
#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AliasSuggestionView {
    pub harness: Tag,
    /// Wire form of the ghost key. Pass it back unchanged to `confirm_alias`.
    pub from: String,
    pub from_label: String,
    /// Wire form of the installed skill's key.
    pub to: String,
    pub to_label: String,
    pub target_body: String,
    pub events: u32,
    pub rule: String,
    /// One line the UI shows beside the confirm button.
    pub reason: String,
}

/// A confirmed link. `target_body` is None when the target skill is no longer installed.
#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AliasView {
    pub harness: Tag,
    pub from: String,
    pub from_label: String,
    pub to: String,
    pub to_label: String,
    pub target_body: Option<String>,
}

fn key_label(key: &InvocationKey) -> String {
    match key {
        InvocationKey::Name(n) => n.clone(),
        InvocationKey::SkillFile(p) => path_str(p),
    }
}

fn harness_tag(snap: &Snapshot, h: HarnessId) -> Tag {
    let label = snap.harnesses.iter().find(|i| i.id == h).map_or(h.as_str(), |i| i.label);
    Tag { id: h.as_str().to_owned(), label: label.to_owned() }
}

pub(crate) fn alias_suggestion_views(snap: &Snapshot, suggestions: &[crate::aliases::Suggestion]) -> Vec<AliasSuggestionView> {
    use crate::aliases::{key_to_wire, RuleId};
    suggestions
        .iter()
        .map(|s| {
            let (rule, reason) = match s.rule {
                RuleId::DropPrefix => ("dropPrefix", "Same name once the old plugin prefix is dropped."),
                RuleId::SkillFolder => ("skillFolder", "Same name as the folder this SKILL.md was read from."),
            };
            AliasSuggestionView {
                harness: harness_tag(snap, s.harness),
                from: key_to_wire(&s.from),
                from_label: key_label(&s.from),
                to: key_to_wire(&s.to),
                to_label: key_label(&s.to),
                target_body: s.target_body.as_str().to_owned(),
                events: s.events,
                rule: rule.to_owned(),
                reason: reason.to_owned(),
            }
        })
        .collect()
}

pub(crate) fn alias_views(snap: &Snapshot, store: &crate::aliases::AliasStore) -> Vec<AliasView> {
    use crate::aliases::key_to_wire;
    store
        .aliases()
        .iter()
        .map(|a| AliasView {
            harness: harness_tag(snap, a.harness),
            from: key_to_wire(&a.from),
            from_label: key_label(&a.from),
            to: key_to_wire(&a.to),
            to_label: key_label(&a.to),
            target_body: snap.resolve(a.harness, &a.to).map(|b| b.as_str().to_owned()),
        })
        .collect()
}

// ---------------------------------------------------------------- actions

/// A change the UI asks for. It names things only by ids the core issued, never by path.
#[derive(Deserialize, Type, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ActionRequest {
    RemoveMount { mount: String },
    DeleteBody { skill: String },
    Disable { mount: String },
    Restore { shelf: String },
    BundleEnable { bundle: String },
    BundleDisable { bundle: String },
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: String,
    pub title: String,
    /// The confirm button's label.
    pub confirm: String,
    pub steps: Vec<StepView>,
    pub effects: Vec<EffectView>,
    pub blockers: Vec<BlockerView>,
    /// No blockers and at least one step. The confirm button is enabled only then.
    pub ready: bool,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub kind: StepKind,
    pub text: String,
    /// The path or command the step works on.
    pub detail: String,
}

#[derive(Serialize, Type, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    RemoveLink,
    Recycle,
    Stash,
    Relink,
    Unstash,
    Run,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectView {
    pub harness: Option<Tag>,
    pub text: String,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BlockerView {
    pub kind: BlockerKind,
    pub reason: String,
    pub remedy: String,
}

#[derive(Serialize, Type, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum BlockerKind {
    RecreatedBy,
    PartOfBundle,
    SharedBody,
    InsideJstackRepo,
    HarnessBuiltin,
    Unsupported,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub outcome: ApplyOutcomeKind,
    /// One per planned step, in order, so the UI shows which ran.
    pub steps: Vec<StepRunView>,
    /// Why a stale plan was refused.
    pub reasons: Vec<String>,
    /// Output of any command the plan ran.
    pub output: Vec<String>,
    /// The inventory revision after the refresh that follows an apply.
    pub revision: u32,
}

#[derive(Serialize, Type, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ApplyOutcomeKind {
    Applied,
    /// A step failed. The steps before it ran, the ones after did not.
    Failed,
    /// The disk changed since planning. Nothing ran, and the UI plans again.
    Stale,
    Refused,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StepRunView {
    pub text: String,
    pub detail: String,
    pub result: StepResultView,
    pub message: Option<String>,
}

#[derive(Serialize, Type, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum StepResultView {
    Done,
    Skipped,
    Failed,
    NotRun,
}

/// Resolves the request's ids against the snapshot. An id the snapshot does not know is an error.
pub(crate) fn parse_action(snap: &Snapshot, req: &ActionRequest) -> Result<ManageAction, CoreError> {
    let unknown = |id: &str| CoreError::UnknownId { id: id.to_owned() };
    let mount = |id: &str| snap.mounts.keys().find(|m| m.as_str() == id).cloned().ok_or_else(|| unknown(id));
    let bundle = |id: &str| snap.bundles.keys().find(|b| b.as_str() == id).cloned().ok_or_else(|| unknown(id));
    Ok(match req {
        ActionRequest::RemoveMount { mount: id } => ManageAction::RemoveMount(mount(id)?),
        ActionRequest::Disable { mount: id } => ManageAction::Disable(mount(id)?),
        ActionRequest::DeleteBody { skill } => {
            ManageAction::DeleteBody(snap.bodies.keys().find(|b| b.as_str() == skill).cloned().ok_or_else(|| unknown(skill))?)
        }
        ActionRequest::Restore { shelf } => {
            ManageAction::Restore(snap.shelf.iter().find(|e| e.id.as_str() == shelf).map(|e| e.id.clone()).ok_or_else(|| unknown(shelf))?)
        }
        ActionRequest::BundleEnable { bundle: id } => ManageAction::Bundle(bundle(id)?, BundleOp::Enable),
        ActionRequest::BundleDisable { bundle: id } => ManageAction::Bundle(bundle(id)?, BundleOp::Disable),
    })
}

pub(crate) fn plan_view(snap: &Snapshot, id: String, plan: &Plan) -> PlanView {
    let (tallies, tracking) = (Tallies::new(), TrackingSince::new());
    let cx = Cx::new(snap, &tallies, &tracking);
    let (title, confirm) = cx.action_title(&plan.action);
    PlanView {
        id,
        title,
        confirm,
        steps: plan.steps.iter().map(step_view).collect(),
        effects: plan.effects.iter().map(|e| cx.effect_view(e)).collect(),
        blockers: plan.blockers.iter().map(|b| cx.blocker_view(b)).collect(),
        ready: plan.blockers.is_empty() && !plan.steps.is_empty(),
    }
}

pub(crate) fn apply_report(plan: &Plan, outcome: ApplyOutcome, revision: u32) -> ApplyReport {
    let run = |results: Vec<StepResult>| -> Vec<StepRunView> {
        plan.steps
            .iter()
            .zip(results)
            .map(|(s, r)| {
                let v = step_view(s);
                let (result, message) = match r {
                    StepResult::Done => (StepResultView::Done, None),
                    StepResult::Skipped => (StepResultView::Skipped, Some("Already done.".to_owned())),
                    StepResult::Failed(m) => (StepResultView::Failed, Some(m)),
                    StepResult::NotRun => (StepResultView::NotRun, None),
                };
                StepRunView { text: v.text, detail: v.detail, result, message }
            })
            .collect()
    };
    let empty = |outcome| ApplyReport { outcome, steps: Vec::new(), reasons: Vec::new(), output: Vec::new(), revision };
    match outcome {
        ApplyOutcome::Refused => empty(ApplyOutcomeKind::Refused),
        ApplyOutcome::Stale { reasons } => ApplyReport { reasons, ..empty(ApplyOutcomeKind::Stale) },
        ApplyOutcome::Applied { results, output } => {
            let failed = results.iter().any(|r| matches!(r, StepResult::Failed(_)));
            let kind = if failed { ApplyOutcomeKind::Failed } else { ApplyOutcomeKind::Applied };
            ApplyReport { steps: run(results), output, ..empty(kind) }
        }
    }
}

fn step_view(s: &Step) -> StepView {
    let arrow = |a: &Path, b: &Path| format!("{} → {}", path_str(a), path_str(b));
    let (kind, text, detail) = match s {
        Step::RemoveLink { link, target, shelve } => (
            StepKind::RemoveLink,
            if shelve.is_some() { "Remove the link and keep it on the shelf" } else { "Remove the link" },
            arrow(link.as_path(), target),
        ),
        Step::Recycle { dir, .. } => (StepKind::Recycle, "Move the folder to the Recycle Bin", path_str(dir.as_path())),
        Step::Stash { dir, stash, .. } => (StepKind::Stash, "Move the folder onto skilltab's shelf", arrow(dir.as_path(), stash)),
        Step::Relink { entry, target, .. } => (StepKind::Relink, "Recreate the link", arrow(entry, target)),
        Step::Unstash { stash, to, .. } => (StepKind::Unstash, "Move the folder back from the shelf", arrow(stash, to)),
        Step::Run { command } => (StepKind::Run, "Run the harness's own command", command.display()),
    };
    StepView { kind, text: text.to_owned(), detail }
}

impl Cx<'_> {
    /// (harness tag, skill name, entry) of a mount, for sentences about it.
    fn mount_words(&self, id: &MountId) -> (Option<Tag>, String, String) {
        match self.snap.mounts.get(id) {
            Some(m) => {
                let name = self.snap.bodies.get(&m.body).map_or_else(String::new, |b| b.manifest.name.clone());
                (Some(self.tag(m.harness)), name, path_str(&m.entry))
            }
            None => (None, String::new(), id.as_str().to_owned()),
        }
    }

    fn action_title(&self, a: &ManageAction) -> (String, String) {
        let harness_label = |t: &Option<Tag>| t.as_ref().map_or_else(|| "this harness".to_owned(), |t| t.label.clone());
        match a {
            ManageAction::RemoveMount(m) => {
                let (h, name, _) = self.mount_words(m);
                (format!("Remove {name} from {}", harness_label(&h)), "Remove".into())
            }
            ManageAction::Disable(m) => {
                let (h, name, _) = self.mount_words(m);
                (format!("Disable {name} in {}", harness_label(&h)), "Disable".into())
            }
            ManageAction::DeleteBody(b) => {
                let name = self.snap.bodies.get(b).map_or("this skill", |b| b.manifest.name.as_str());
                (format!("Delete {name} everywhere"), "Move to Recycle Bin".into())
            }
            ManageAction::Restore(id) => match self.snap.shelf.iter().find(|e| &e.id == id) {
                Some(e) => (format!("Restore {} in {}", e.name, self.tag(e.harness).label), "Restore".into()),
                None => ("Restore".into(), "Restore".into()),
            },
            ManageAction::Bundle(b, op) => {
                let verb = if *op == BundleOp::Enable { "Enable" } else { "Disable" };
                (format!("{verb} plugin {}", self.bundle_name(b)), format!("{verb} plugin"))
            }
        }
    }

    fn effect_view(&self, e: &Effect) -> EffectView {
        match e {
            Effect::HarnessLosesSkill { mount } => {
                let (harness, name, entry) = self.mount_words(mount);
                let who = harness.as_ref().map_or("The harness", |t| t.label.as_str()).to_owned();
                EffectView { text: format!("{who} stops loading {name} from {entry}."), harness }
            }
            Effect::HarnessGainsSkill { mount } => {
                let (harness, name, entry) = self.mount_words(mount);
                let who = harness.as_ref().map_or("The harness", |t| t.label.as_str()).to_owned();
                EffectView { text: format!("{who} loads {name} from {entry} again."), harness }
            }
            Effect::AlsoBreaks { mount, via } => {
                let (harness, _, entry) = self.mount_words(mount);
                let who = harness.as_ref().map_or("another harness", |t| t.label.as_str()).to_owned();
                EffectView { text: format!("Also breaks {who}'s mount at {entry}, which goes through {}.", path_str(via)), harness }
            }
            Effect::BodyRecycled { dir } => {
                EffectView { harness: None, text: format!("The folder {} and everything in it goes to the Recycle Bin.", path_str(dir)) }
            }
        }
    }

    fn blocker_view(&self, b: &Blocker) -> BlockerView {
        let (kind, reason, remedy) = match b {
            Blocker::RecreatedBy { owner: Ownership::JstackSync, name } => (
                BlockerKind::RecreatedBy,
                "jstack's sync recreates this link at the next session start, so removing it would not last.".to_owned(),
                format!("Add {name} to jstack/exclude.txt so the sync stops linking it, then rescan."),
            ),
            Blocker::RecreatedBy { owner: Ownership::ClaudeAiSync, .. } => (
                BlockerKind::RecreatedBy,
                "claude.ai sync writes this skill back on its next sync.".to_owned(),
                "Remove the skill from your claude.ai account.".to_owned(),
            ),
            Blocker::RecreatedBy { owner, .. } => (
                BlockerKind::RecreatedBy,
                format!("{} puts this entry back.", owner_tag(owner).label),
                "Remove it with the tool that made it.".to_owned(),
            ),
            Blocker::PartOfBundle { bundle } => (
                BlockerKind::PartOfBundle,
                format!("This skill ships inside plugin {}, and its plugin manager owns the folder.", self.bundle_name(bundle)),
                "Turn the whole plugin off with Disable plugin.".to_owned(),
            ),
            Blocker::SharedBody { dependents } => {
                let who: Vec<String> = dependents
                    .iter()
                    .map(|d| {
                        let (h, _, entry) = self.mount_words(d);
                        format!("{} at {entry}", h.map_or_else(String::new, |t| t.label))
                    })
                    .collect();
                (
                    BlockerKind::SharedBody,
                    format!("This is the real folder, and {} still reach it through links.", who.join(", ")),
                    "Use Delete everywhere to remove it for every harness, or remove those links first.".to_owned(),
                )
            }
            Blocker::InsideJstackRepo => (
                BlockerKind::InsideJstackRepo,
                "This folder is your own source in the jstack clone.".to_owned(),
                "Remove the links that point at it, or delete it in the jstack repo with git.".to_owned(),
            ),
            Blocker::HarnessBuiltin { harness } => {
                let label = self.tag(*harness).label;
                (
                    BlockerKind::HarnessBuiltin,
                    format!("This ships with {label}, which puts it back on every update."),
                    format!("Turn it off in {label}'s own settings, if it allows that."),
                )
            }
            Blocker::Unsupported { why, remedy } => (BlockerKind::Unsupported, why.clone(), remedy.clone()),
        };
        BlockerView { kind, reason, remedy }
    }
}

// ---------------------------------------------------------------- SKILL.md

fn read_skill_md(dir: &Path) -> (String, bool) {
    let Ok(file) = std::fs::File::open(dir.join("SKILL.md")) else { return (String::new(), false) };
    let mut buf = Vec::new();
    let _ = file.take(SKILL_MD_LIMIT + 1).read_to_end(&mut buf);
    let truncated = buf.len() as u64 > SKILL_MD_LIMIT;
    buf.truncate(SKILL_MD_LIMIT as usize);
    (String::from_utf8_lossy(&buf).into_owned(), truncated)
}

/// Splits a leading `---` block into top-level fields and returns the rest as the markdown body. A continuation
/// line (indented or blank) belongs to the field above it. Block scalars (`>`, `|`) lose their indicator.
fn split_frontmatter(text: &str) -> (Vec<Field>, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (Vec::new(), text.to_owned());
    }
    let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1) else {
        return (Vec::new(), text.to_owned());
    };

    let mut fields: Vec<(String, String, Vec<String>)> = Vec::new();
    for line in &lines[1..end] {
        let key_value = if line.starts_with([' ', '\t']) { None } else { line.split_once(':').filter(|(k, _)| is_key(k)) };
        match key_value {
            Some((k, v)) => fields.push((k.to_owned(), v.trim().to_owned(), Vec::new())),
            None => {
                if let Some(last) = fields.last_mut() {
                    last.2.push(line.trim().to_owned());
                }
            }
        }
    }

    let out = fields
        .into_iter()
        .map(|(key, first, cont)| {
            let cont: Vec<String> = {
                let mut c = cont;
                while c.last().is_some_and(|l| l.is_empty()) {
                    c.pop();
                }
                c
            };
            let value = if cont.is_empty() {
                unquote(&first)
            } else {
                let block = matches!(first.as_str(), ">" | ">-" | ">+" | "|" | "|-" | "|+");
                let mut parts: Vec<String> = Vec::new();
                if !block && !first.is_empty() {
                    parts.push(first);
                }
                parts.extend(cont);
                let sep = if block && !parts.is_empty() { "\n" } else { " " };
                parts.join(sep)
            };
            Field { key, value }
        })
        .collect();

    let body = lines[end + 1..].join("\n");
    (out, body.trim_start_matches('\n').to_owned())
}

fn is_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return t[1..t.len() - 1].to_owned();
        }
    }
    t.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_becomes_fields_and_the_body_is_left_over() {
        let text = "---\nname: tdd\ndescription: >\n  Test first.\n  Always.\nmodel: \"opus\"\n---\n\n# Title\nbody";
        let (fields, body) = split_frontmatter(text);
        let get = |k: &str| fields.iter().find(|f| f.key == k).map(|f| f.value.as_str());
        assert_eq!(get("name"), Some("tdd"));
        assert_eq!(get("description"), Some("Test first.\nAlways."));
        assert_eq!(get("model"), Some("opus"));
        assert_eq!(body, "# Title\nbody");
    }

    #[test]
    fn text_without_a_closed_frontmatter_block_is_all_body() {
        let (fields, body) = split_frontmatter("---\nname: x\nno end");
        assert!(fields.is_empty());
        assert!(body.starts_with("---"));
    }

    #[test]
    fn verbatim_prefix_is_dropped_from_shown_paths() {
        assert_eq!(path_str(Path::new(r"\\?\C:\a\b")), r"C:\a\b");
        assert_eq!(path_str(Path::new(r"\\?\UNC\srv\share")), r"\\srv\share");
        assert_eq!(path_str(Path::new(r"C:\a")), r"C:\a");
    }
}
