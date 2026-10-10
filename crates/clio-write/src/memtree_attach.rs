// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree structural leaf attach and fan-out splits.
//!
//! ## Owns
//! - Time-ordered leaf insert into a bank tree
//! - k-ary split on overflow
//! - Ancestor dirty marking on attach
//!
//! ## Does not own
//! - Parallel summary refresh or tool handlers
//!
//! ## Boundary
//!
//! Changes to attach / split topology should normally require changes here.
//! Changes to refresh scheduling should not require changes here.
//!
//! Keep this module focused on structural MemTree insert.

use crate::memtree::{AttachLeaf, BankMemTree, MemTreeNode};
use clio_types::{AmError, ErrorCode};

impl BankMemTree {
    /// Attach a leaf in time order; mark ancestors dirty; split on fan-out overflow.
    ///
    /// Returns the new leaf node id and the ancestor ids marked dirty.
    pub fn attach_leaf(&mut self, leaf: AttachLeaf) -> Result<(String, Vec<String>), AmError> {
        if leaf.bank_id != self.bank_id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "cross-bank memtree attach rejected",
            ));
        }
        if self.placement.contains_key(&leaf.item_id) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("item `{}` already placed in memtree", leaf.item_id),
            ));
        }

        let leaf_id = self.next_id("leaf");
        let node = MemTreeNode {
            id: leaf_id.clone(),
            bank_id: self.bank_id.clone(),
            parent_id: None,
            item_id: Some(leaf.item_id.clone()),
            depth: 0,
            interval_start: leaf.interval_start.clone(),
            interval_end: leaf.interval_end.clone(),
            child_ids: vec![],
            summary_gist: leaf.gist.clone(),
            dirty: false,
            generation: 0,
            refreshing: false,
            task_id: leaf.task_id.clone(),
            last_error: None,
        };
        self.nodes.insert(leaf_id.clone(), node);
        self.placement.insert(leaf.item_id, leaf_id.clone());

        if self.root_id.is_none() {
            let root_id = self.next_id("int");
            {
                let leaf_node = self.nodes.get_mut(&leaf_id).expect("leaf just inserted");
                leaf_node.parent_id = Some(root_id.clone());
            }
            let root = MemTreeNode {
                id: root_id.clone(),
                bank_id: self.bank_id.clone(),
                parent_id: None,
                item_id: None,
                depth: 1,
                interval_start: leaf.interval_start,
                interval_end: leaf.interval_end,
                child_ids: vec![leaf_id.clone()],
                summary_gist: None,
                dirty: true,
                generation: 1,
                refreshing: false,
                task_id: leaf.task_id,
                last_error: None,
            };
            self.nodes.insert(root_id.clone(), root);
            self.root_id = Some(root_id.clone());
            return Ok((leaf_id, vec![root_id]));
        }

        let parent_id = self.pick_parent_for_leaf(&leaf_id)?;
        self.insert_child_sorted(&parent_id, &leaf_id);
        let split_dirty = self.split_upwards(&parent_id);
        let mut dirty = self.mark_ancestors_dirty(&leaf_id);
        dirty.extend(split_dirty);
        dirty.sort();
        dirty.dedup();
        Ok((leaf_id, dirty))
    }

    fn pick_parent_for_leaf(&self, leaf_id: &str) -> Result<String, AmError> {
        let leaf = self.nodes.get(leaf_id).expect("leaf present");
        let mut candidates: Vec<&MemTreeNode> = self
            .nodes
            .values()
            .filter(|n| n.depth == 1 && n.item_id.is_none())
            .collect();
        candidates.sort_by(|a, b| a.interval_start.cmp(&b.interval_start));
        if candidates.is_empty() {
            return self
                .root_id
                .clone()
                .ok_or_else(|| AmError::new(ErrorCode::Internal, "memtree root missing"));
        }
        let mut chosen = candidates[0].id.clone();
        for c in &candidates {
            if c.interval_start.as_str() <= leaf.interval_start.as_str() {
                chosen.clone_from(&c.id);
            } else {
                break;
            }
        }
        Ok(chosen)
    }

    fn insert_child_sorted(&mut self, parent_id: &str, child_id: &str) {
        let child_start = self
            .nodes
            .get(child_id)
            .expect("child present for insert")
            .interval_start
            .clone();
        let child_ids = self
            .nodes
            .get(parent_id)
            .expect("parent present for insert")
            .child_ids
            .clone();
        let mut insert_at = child_ids.len();
        for (i, cid) in child_ids.iter().enumerate() {
            let start = self
                .nodes
                .get(cid)
                .map_or("", |n| n.interval_start.as_str());
            if child_start.as_str() < start {
                insert_at = i;
                break;
            }
        }
        let parent = self.nodes.get_mut(parent_id).expect("parent");
        parent.child_ids.insert(insert_at, child_id.to_owned());
        self.nodes.get_mut(child_id).expect("child").parent_id = Some(parent_id.to_owned());
        self.refresh_interval_from_children(parent_id);
    }

    pub(crate) fn refresh_interval_from_children(&mut self, node_id: &str) {
        let child_ids = self
            .nodes
            .get(node_id)
            .map(|n| n.child_ids.clone())
            .unwrap_or_default();
        if child_ids.is_empty() {
            return;
        }
        let mut start = None;
        let mut end = None;
        for cid in &child_ids {
            if let Some(c) = self.nodes.get(cid) {
                start = Some(match start {
                    None => c.interval_start.clone(),
                    Some(s) if c.interval_start < s => c.interval_start.clone(),
                    Some(s) => s,
                });
                end = Some(match end {
                    None => c.interval_end.clone(),
                    Some(e) if c.interval_end > e => c.interval_end.clone(),
                    Some(e) => e,
                });
            }
        }
        if let Some(n) = self.nodes.get_mut(node_id) {
            if let Some(s) = start {
                n.interval_start = s;
            }
            if let Some(e) = end {
                n.interval_end = e;
            }
        }
    }

    fn split_upwards(&mut self, start_id: &str) -> Vec<String> {
        let mut current_id = start_id.to_owned();
        let mut split_dirty = Vec::new();
        loop {
            let (depth, parent_id, child_ids) = {
                let Some(n) = self.nodes.get(&current_id) else {
                    return split_dirty;
                };
                if n.depth == 0 || n.child_ids.len() <= self.fanout {
                    return split_dirty;
                }
                (n.depth, n.parent_id.clone(), n.child_ids.clone())
            };
            let split_at = child_ids.len() / 2;
            let right_ids: Vec<String> = child_ids[split_at..].to_vec();
            {
                let n = self.nodes.get_mut(&current_id).expect("current");
                n.child_ids.truncate(split_at);
            }
            self.refresh_interval_from_children(&current_id);

            let sibling_id = self.next_id("int");
            let sib_start = self
                .nodes
                .get(&right_ids[0])
                .expect("right child")
                .interval_start
                .clone();
            for rid in &right_ids {
                self.nodes.get_mut(rid).expect("child").parent_id = Some(sibling_id.clone());
            }
            let sibling = MemTreeNode {
                id: sibling_id.clone(),
                bank_id: self.bank_id.clone(),
                parent_id: parent_id.clone(),
                item_id: None,
                depth,
                interval_start: sib_start.clone(),
                interval_end: sib_start,
                child_ids: right_ids,
                summary_gist: None,
                dirty: true,
                generation: 1,
                refreshing: false,
                task_id: None,
                last_error: None,
            };
            self.nodes.insert(sibling_id.clone(), sibling);
            split_dirty.push(sibling_id.clone());
            // Sibling interval must cover every assigned child's true span, not
            // only the last child's interval_end (an earlier child can extend further).
            self.refresh_interval_from_children(&sibling_id);
            self.nodes.get_mut(&current_id).expect("current").dirty = true;

            if self.root_id.as_deref() == Some(current_id.as_str()) {
                let new_root_id = self.next_id("int");
                let (rs, re) = {
                    let a = self.nodes.get(&current_id).expect("left");
                    let b = self.nodes.get(&sibling_id).expect("right");
                    (
                        a.interval_start.clone().min(b.interval_start.clone()),
                        a.interval_end.clone().max(b.interval_end.clone()),
                    )
                };
                self.nodes.get_mut(&current_id).expect("left").parent_id =
                    Some(new_root_id.clone());
                self.nodes.get_mut(&sibling_id).expect("right").parent_id =
                    Some(new_root_id.clone());
                let new_root = MemTreeNode {
                    id: new_root_id.clone(),
                    bank_id: self.bank_id.clone(),
                    parent_id: None,
                    item_id: None,
                    depth: depth + 1,
                    interval_start: rs,
                    interval_end: re,
                    child_ids: vec![current_id, sibling_id],
                    summary_gist: None,
                    dirty: true,
                    generation: 1,
                    refreshing: false,
                    task_id: None,
                    last_error: None,
                };
                self.nodes.insert(new_root_id.clone(), new_root);
                self.root_id = Some(new_root_id);
                return split_dirty;
            }

            let parent_id = parent_id.expect("split parent present for non-root");
            let parent = self.nodes.get_mut(&parent_id).expect("parent");
            if let Some(pos) = parent.child_ids.iter().position(|c| c == &current_id) {
                parent.child_ids.insert(pos + 1, sibling_id);
            } else {
                parent.child_ids.push(sibling_id);
            }
            self.refresh_interval_from_children(&parent_id);
            current_id = parent_id;
        }
    }

    pub(crate) fn mark_ancestors_dirty(&mut self, node_id: &str) -> Vec<String> {
        let mut dirty = Vec::new();
        let mut cur = self.nodes.get(node_id).and_then(|n| n.parent_id.clone());
        while let Some(id) = cur.take() {
            if let Some(n) = self.nodes.get_mut(&id) {
                n.dirty = true;
                n.generation = n.generation.saturating_add(1);
                n.last_error = None;
                dirty.push(id);
                cur.clone_from(&n.parent_id);
            } else {
                break;
            }
        }
        dirty
    }
}
