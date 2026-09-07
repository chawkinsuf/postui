//! Spaces: the `spaces` list and `[space.<slug>]` tables in
//! `project.toml`, the directories under `requests/`, and the local
//! memory keyed by space.

use super::*;
use crate::journal::MergeKey;

impl Project {
    /// `meta.spaces` exactly as written (duplicates dropped, invalid
    /// names kept in place) then unlisted directories: the list every
    /// space op writes back. Never filters a hand-written entry away.
    fn write_list(&mut self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for name in &self.meta.spaces {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
        let (listed, _) = Self::list_spaces(&mut self.disk, &self.meta);
        for name in listed {
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// Re-lists spaces from `meta` and disk; a vanished active space
    /// falls back to the first with a warning.
    pub(crate) fn refresh_spaces(&mut self) -> Option<Warning> {
        let (spaces, invalid) = Self::list_spaces(&mut self.disk, &self.meta);
        self.spaces = spaces;
        self.spaces_warning = super::join_warnings(&invalid);
        if self.spaces.contains(&self.local.active_space) {
            return None;
        }
        let gone = std::mem::take(&mut self.local.active_space);
        let gone = self.space_name(&gone);
        self.local.active_space = self
            .spaces
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_SPACE.to_string());
        Some(format!("space {gone:?} no longer exists"))
    }

    /// `(old, new)` for every request under space `from`, at any folder
    /// level — what a rename of the space's directory does to slugs.
    fn space_moves(
        listing: &[RequestListing],
        from: &str,
        to: &str,
    ) -> Vec<(String, String)> {
        let prefix = format!("{from}/");
        listing
            .iter()
            .filter(|l| crate::storage::space_of(&l.slug) == Some(from))
            .map(|l| {
                (
                    l.slug.clone(),
                    l.slug.replacen(&prefix, &format!("{to}/"), 1),
                )
            })
            .collect()
    }

    pub fn space_slug_for(&self, display: &str, exclude: Option<&str>) -> String {
        let listed = self.meta.spaces.clone();
        meta::unique_slug_among(
            meta::Kind::Space,
            display,
            |slug| {
                listed.iter().any(|s| s == slug)
                    || self.spaces.iter().any(|s| s == slug)
                    || space_rel(slug).map(|p| self.disk.exists(&p)).unwrap_or(false)
            },
            exclude,
        )
    }

    pub fn create_space(&mut self, display: &str) -> Result<String, Error> {
        let display = meta::display_name_of(display)?;
        let mut spaces = self.write_list();
        let meta = self.meta.clone();
        if meta::display_taken(&display, &spaces, |s| meta::space_display(&meta, s), None) {
            return Err(Error::AlreadyExists(display));
        }
        let slug = self.space_slug_for(&display, None);
        spaces.push(slug.clone());
        self.transaction("create space", EntryMeta::default(), |p| {
            p.fs_create_dir(&space_rel(&slug)?)?;
            p.edit_project_toml(|doc| {
                doc["spaces"] = toml_edit::value(meta::slug_array(&spaces));
                meta::set_item_name(doc, meta::Kind::Space, &slug, &display);
            })?;
            p.refresh_spaces();
            Ok(())
        })?;
        Ok(slug)
    }

    /// Renames `from` (a slug) to the display name `display`: the dir is
    /// re-slugged, the list entry rewritten in place, the table moved,
    /// and local memory (active space, open request, expanded) re-keyed
    /// and persisted, all in one entry.
    pub fn rename_space(&mut self, from: &str, display: &str) -> Result<String, Error> {
        let display = meta::display_name_of(display)?;
        let mut spaces = self.write_list();
        let Some(idx) = spaces.iter().position(|s| s == from) else {
            return Err(Error::NotFound(from.to_string()));
        };
        let meta = self.meta.clone();
        if meta::display_taken(&display, &spaces, |s| meta::space_display(&meta, s), Some(from)) {
            return Err(Error::AlreadyExists(display));
        }
        let to = self.space_slug_for(&display, Some(from));
        spaces[idx] = to.clone();
        let from = from.to_string();
        // Every request in the space changes slug with the directory, at
        // every folder level: the entry says so, so an undo or redo can
        // take the open request (and the app's session cache) back with
        // it, exactly as a request move does.
        let meta = EntryMeta {
            moves: if to != from {
                Self::space_moves(&self.listing, &from, &to)
            } else {
                Vec::new()
            },
            ..EntryMeta::default()
        };
        self.transaction("rename space", meta, |p| {
            let from_dir = space_rel(&from)?;
            let to_dir = space_rel(&to)?;
            if to != from && p.disk.is_dir(&from_dir) {
                p.fs_rename(&from_dir, &to_dir)?;
            } else if !p.disk.is_dir(&to_dir) {
                p.fs_create_dir(&to_dir)?;
            }
            p.edit_project_toml(|doc| {
                doc["spaces"] = toml_edit::value(meta::slug_array(&spaces));
                meta::move_item_table(doc, meta::Kind::Space, &from, &to);
                meta::set_item_name(doc, meta::Kind::Space, &to, &display);
            })?;
            if to != from {
                p.rename_space_local(&from, &to);
                let moved: Vec<(String, String)> = p
                    .open_requests
                    .keys()
                    .filter(|s| crate::storage::space_of(s) == Some(from.as_str()))
                    .map(|s| (s.clone(), s.replacen(&format!("{from}/"), &format!("{to}/"), 1)))
                    .collect();
                for (old, new) in moved {
                    if let Some(req) = p.open_requests.shift_remove(&old) {
                        p.open_requests.insert(new, req);
                    }
                }
                p.persist_local_journaled()?;
            }
            p.refresh_spaces();
            p.relist();
            Ok(())
        })?;
        Ok(to)
    }

    /// `persist_local` as a journaled text op, for the cascades whose
    /// undo must put local memory back exactly.
    pub(crate) fn persist_local_journaled(&mut self) -> Result<(), Error> {
        let text = self.local_state_text();
        self.fs_write_text(&rel(STATE_TOML)?, Some(&text))
    }

    /// Trashes the directory (if any), drops the entry and table, forgets
    /// the space's local memory. Refuses the only displayable space.
    pub fn delete_space(&mut self, name: &str) -> Result<(), Error> {
        let mut spaces = self.write_list();
        let Some(idx) = spaces.iter().position(|s| s == name) else {
            return Err(Error::NotFound(name.to_string()));
        };
        if spaces.iter().filter(|s| meta::valid_space_name(s)).count() == 1 {
            return Err(Error::LastSpace);
        }
        spaces.remove(idx);
        let name = name.to_string();
        let meta = EntryMeta {
            reopen: self.local.open_request.clone(),
            ..EntryMeta::default()
        };
        self.transaction("delete space", meta, |p| {
            p.edit_project_toml(|doc| {
                doc["spaces"] = toml_edit::value(meta::slug_array(&spaces));
                meta::remove_item_table(doc, meta::Kind::Space, &name);
            })?;
            let dir = space_rel(&name)?;
            if p.disk.is_dir(&dir) {
                p.fs_trash(&dir)?;
            }
            p.open_requests
                .retain(|slug, _| crate::storage::space_of(slug) != Some(name.as_str()));
            p.forget_space_local(&name);
            if p.local.open_request.as_deref().and_then(crate::storage::space_of) == Some(name.as_str()) {
                p.local.open_request = None;
            }
            p.refresh_spaces();
            p.persist_local_journaled()?;
            p.relist();
            Ok(())
        })
    }

    /// Moves `name` by `delta` among the displayed spaces (a swap).
    pub fn move_space(&mut self, name: &str, delta: i32) -> Result<Option<ListChange>, Error> {
        let mut spaces = self.write_list();
        let slots: Vec<usize> = (0..spaces.len())
            .filter(|i| meta::valid_space_name(&spaces[*i]))
            .collect();
        let Some(pos) = slots.iter().position(|i| spaces[*i] == *name) else {
            return Err(Error::NotFound(name.to_string()));
        };
        let target = (pos as i32 + delta).clamp(0, slots.len() as i32 - 1) as usize;
        if target == pos {
            return Ok(None);
        }
        let before = meta::displayed_spaces(&spaces);
        spaces.swap(slots[pos], slots[target]);
        let after = meta::displayed_spaces(&spaces);
        let key = MergeKey::SpaceOrder { name: name.to_string() };
        self.transaction_merging("move space", key, |p| {
            p.edit_project_toml(|doc| doc["spaces"] = toml_edit::value(meta::slug_array(&spaces)))?;
            p.refresh_spaces();
            Ok(())
        })?;
        Ok(Some(ListChange { before, after }))
    }

    /// Writes the whole displayed order as a drag left it. Refuses an
    /// order that is not exactly the displayed set.
    pub fn set_space_order(&mut self, displayed: &[String]) -> Result<Option<ListChange>, Error> {
        let mut spaces = self.write_list();
        let slots: Vec<usize> = (0..spaces.len())
            .filter(|i| meta::valid_space_name(&spaces[*i]))
            .collect();
        let valid: Vec<String> = slots.iter().map(|i| spaces[*i].clone()).collect();
        if let Some(extra) = displayed.iter().find(|n| !valid.contains(n)) {
            return Err(Error::NotFound(extra.clone()));
        }
        if let Some(missing) = valid.iter().find(|n| !displayed.contains(n)) {
            return Err(Error::NotFound(missing.clone()));
        }
        if displayed.len() != valid.len() {
            return Err(Error::NotFound(displayed[0].clone()));
        }
        if valid == displayed {
            return Ok(None);
        }
        for (slot, name) in slots.iter().zip(displayed) {
            spaces[*slot] = name.clone();
        }
        self.transaction("reorder spaces", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| doc["spaces"] = toml_edit::value(meta::slug_array(&spaces)))?;
            p.refresh_spaces();
            Ok(())
        })?;
        Ok(Some(ListChange { before: valid, after: displayed.to_vec() }))
    }

    /// Rewrites one level of `space`'s request order to `shown`
    /// (`order::set_level_order`'s merge rule). A drag: never merges.
    pub fn set_request_order(&mut self, space: &str, level: &str, shown: &[String]) -> Result<Option<ListChange>, Error> {
        let before = crate::order::space_order(&self.meta, space).to_vec();
        let after = crate::order::merge_level(&before, level, shown);
        if before == after {
            return Ok(None);
        }
        self.transaction("reorder requests", EntryMeta::default(), |p| {
            p.order_edit(space, |order| {
                *order = after.clone();
                Vec::new()
            })
            .map(|_| ())
        })?;
        Ok(Some(ListChange { before, after }))
    }

    /// alt+↑/↓: `rel` moves by `delta` among `shown`; a burst merges.
    pub fn move_request_shown(&mut self, space: &str, level: &str, shown: &[String], rel: &str, delta: i32) -> Result<Option<ListChange>, Error> {
        let Some(pos) = shown.iter().position(|s| s == rel) else {
            return Err(Error::NotFound(format!("{space}/{rel}")));
        };
        let target = (pos as i32 + delta).clamp(0, shown.len() as i32 - 1) as usize;
        if target == pos {
            return Ok(None);
        }
        let mut shown = shown.to_vec();
        let moved = shown.remove(pos);
        shown.insert(target, moved);
        let before = crate::order::space_order(&self.meta, space).to_vec();
        let after = crate::order::merge_level(&before, level, &shown);
        if before == after {
            return Ok(None);
        }
        let key = MergeKey::RequestOrder { space: space.to_string(), slug: format!("{space}/{rel}") };
        self.transaction_merging("move request", key, |p| {
            p.order_edit(space, |order| {
                *order = after.clone();
                Vec::new()
            })
            .map(|_| ())
        })?;
        Ok(Some(ListChange { before, after }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    #[test]
    fn create_slugifies_records_the_name_and_refuses_a_taken_display_name() {
        let (dir, mut p) = fixture();
        let slug = p.create_space("Billing & Co").unwrap();
        assert_eq!(slug, "billing-co");
        assert_eq!(p.spaces(), ["main", "auth", "billing-co"]);
        assert_eq!(p.space_name("billing-co"), "Billing & Co");
        assert!(dir.path().join("requests/billing-co").is_dir());
        assert!(matches!(p.create_space("auth"), Err(Error::AlreadyExists(_))));
        assert!(matches!(p.create_space("  "), Err(Error::BadName(_))));
        assert_eq!(p.journal_len(), 1);
    }

    #[test]
    fn rename_reslugs_the_dir_moves_the_table_and_re_keys_local_memory() {
        let (dir, mut p) = fixture();
        p.set_active_space("auth");
        p.record_space_open(Some("auth/login"));
        p.persist_local().unwrap();
        let to = p.rename_space("auth", "Login Flow").unwrap();
        assert_eq!(to, "login-flow");
        assert!(dir.path().join("requests/login-flow/login.toml").is_file());
        assert_eq!(p.spaces(), ["main", "login-flow"]);
        assert_eq!(p.space_name("login-flow"), "Login Flow");
        assert_eq!(p.local().active_space, "login-flow");
        assert_eq!(p.local().space_open.get("login-flow").map(String::as_str), Some("login-flow/login"));
        let state = read(&dir, ".local/state.toml").unwrap();
        assert!(state.contains("login-flow"), "{state}");
        assert!(read(&dir, "project.toml").unwrap().contains("[space.login-flow]"));
        let e = p.journal.pop_undo().unwrap();
        assert!(e.ops.iter().any(|o| matches!(o, crate::journal::Op::Text { path, .. } if path.as_str() == ".local/state.toml")));
    }

    #[test]
    fn rename_records_a_move_for_every_request_in_the_space() {
        let (dir, mut p) = fixture();
        std::fs::create_dir_all(dir.path().join("requests/main/api")).unwrap();
        std::fs::write(dir.path().join("requests/main/api/deep.toml"), "method = \"GET\"\nurl = \"https://x/d\"\n").unwrap();
        p.reload_all();
        p.rename_space("main", "Core").unwrap();
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.label, "rename space");
        let mut moves = u.meta.moves.clone();
        moves.sort();
        assert_eq!(
            moves,
            vec![
                ("main/api/deep".to_string(), "core/api/deep".to_string()),
                ("main/ping".to_string(), "core/ping".to_string()),
            ],
            "every request in the space, at every folder level"
        );
        assert!(dir.path().join("requests/main/api/deep.toml").is_file());
    }

    #[test]
    fn a_display_only_rename_keeps_the_slug() {
        let (_d, mut p) = fixture();
        assert_eq!(p.rename_space("auth", "AUTH").unwrap(), "auth");
        assert_eq!(p.space_name("auth"), "AUTH");
    }

    #[test]
    fn delete_trashes_the_dir_drops_the_entry_and_refuses_the_last_space() {
        let (dir, mut p) = fixture();
        p.set_active_space("auth");
        p.set_open_request(Some("auth/login"));
        p.delete_space("auth").unwrap();
        assert!(!dir.path().join("requests/auth").exists());
        assert!(dir.path().join(".local/trash/1/requests/auth/login.toml").is_file());
        assert_eq!(p.spaces(), ["main"]);
        assert_eq!(p.local().active_space, "main");
        assert!(p.local().open_request.is_none(), "the open request pointed into the deleted space");
        assert!(!read(&dir, "project.toml").unwrap().contains("[space.auth]"));
        assert!(matches!(p.delete_space("main"), Err(Error::LastSpace)));
    }

    #[test]
    fn move_space_swaps_clamps_and_a_burst_merges_into_one_entry() {
        let (_d, mut p) = fixture();
        assert!(p.move_space("main", -1).unwrap().is_none());
        let c = p.move_space("main", 1).unwrap().unwrap();
        assert_eq!(c.after, ["auth", "main"]);
        p.move_space("main", -1).unwrap();
        assert_eq!(p.spaces(), ["main", "auth"]);
        assert_eq!(
            p.journal_len(),
            0,
            "two keyboard moves within 2 s merge into one step, which nets to identity and is dropped"
        );
    }

    #[test]
    fn set_space_order_writes_the_dragged_order_and_refuses_a_wrong_set() {
        let (_d, mut p) = fixture();
        let c = p.set_space_order(&["auth".into(), "main".into()]).unwrap().unwrap();
        assert_eq!(c.before, ["main", "auth"]);
        assert_eq!(p.spaces(), ["auth", "main"]);
        assert!(p.set_space_order(&["auth".into(), "main".into()]).unwrap().is_none());
        assert!(matches!(p.set_space_order(&["auth".into()]), Err(Error::NotFound(_))));
    }

    #[test]
    fn request_order_edits_write_the_space_table_and_merge_bursts() {
        let (dir, mut p) = fixture();
        for n in ["a", "b", "c"] {
            std::fs::write(dir.path().join(format!("requests/main/{n}.toml")), "method = \"GET\"\nurl = \"u\"\n").unwrap();
        }
        p.relist();
        let shown: Vec<String> = ["a", "b", "c", "ping"].iter().map(|s| s.to_string()).collect();
        p.move_request_shown("main", "", &shown, "c", -1).unwrap();
        let shown: Vec<String> = ["a", "c", "b", "ping"].iter().map(|s| s.to_string()).collect();
        p.move_request_shown("main", "", &shown, "c", -1).unwrap();
        assert_eq!(crate::order::space_order(p.meta(), "main"), ["c", "a", "b", "ping"]);
        assert_eq!(p.journal_len(), 1);
        let c = p.set_request_order("main", "", &["ping".into(), "c".into(), "a".into(), "b".into()]).unwrap().unwrap();
        assert_eq!(c.after, ["ping", "c", "a", "b"]);
        assert_eq!(p.journal_len(), 2, "a drag never merges");
    }

    #[test]
    fn a_failed_space_rename_keeps_the_held_request_open_and_unchanged() {
        let (dir, mut p) = fixture();
        let before = p.open_request("main/ping").unwrap().clone();
        // Make the rename fail *after* the directory move and the held-request
        // re-key: the local-state write at the end of the cascade hits a
        // directory where `.local/state.toml` should be. `state.toml` does
        // not exist yet at this point, so create it first.
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(dir.path().join(".local/state.toml"), "").unwrap();
        std::fs::remove_file(dir.path().join(".local/state.toml")).unwrap();
        std::fs::create_dir(dir.path().join(".local/state.toml")).unwrap();
        assert!(p.rename_space("main", "Renamed").is_err());
        assert_eq!(p.held_request("main/ping"), Some(&before), "still held under its old slug");
        assert!(dir.path().join("requests/main/ping.toml").is_file());
    }
}
