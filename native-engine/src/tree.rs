//! B25: dynamic ordered B+ tree, each logical node stored in one B24 pager slot.
//! A dedicated tree store, not yet the SQL backend.
use crate::{
    pager::{Change, Pager, Report},
    valid_id, Error, NativeEntry, Result,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
const META: &str = "tree_meta";
const NODE: &str = "tree_node";
const DATA: &str = "tree_data";
const ORDER: usize = 8;
const MAX_HEIGHT: usize = 5;
const MAX_NODES: u32 = 1024;
fn invalid() -> Error {
    Error::Invalid("tree: malformed structure")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    root: u32,
    next: u32,
    count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Node {
    Leaf {
        keys: Vec<String>,
        next: Option<u32>,
    },
    Branch {
        keys: Vec<String>,
        children: Vec<u32>,
    },
}
#[derive(Debug, Serialize)]
pub struct TreeReport {
    pub generation: u64,
    pub rows: u32,
    pub nodes: u32,
    pub height: usize,
}
pub struct Tree {
    pager: Pager,
    meta: Meta,
    meta_rev: u64,
    height: usize,
}
fn id(n: u32) -> String {
    format!("n{n}")
}
fn mutation<T: Serialize>(ns: &str, key: String, rev: u64, value: &T) -> Result<Change> {
    Ok(Change {
        namespace: ns.into(),
        key,
        expected_revision: rev,
        value: Some(serde_json::to_string(value).map_err(|_| invalid())?),
    })
}
fn parse<T: serde::de::DeserializeOwned>(r: NativeEntry) -> Result<(T, u64)> {
    let value = r.value.ok_or_else(invalid)?;
    Ok((
        serde_json::from_str(&value).map_err(|_| invalid())?,
        r.revision,
    ))
}
fn keys_valid(keys: &[String]) -> bool {
    keys.len() <= ORDER && keys.iter().all(|k| valid_id(k)) && keys.windows(2).all(|p| p[0] < p[1])
}
struct Walk {
    first: Option<String>,
    last: Option<String>,
    height: usize,
    count: u32,
}
impl Tree {
    pub fn create(path: &Path, slots: u32) -> Result<Self> {
        let mut pager = Pager::create(path, slots)?;
        let meta = Meta {
            root: 1,
            next: 2,
            count: 0,
        };
        pager.commit(&[
            mutation(META, "root".into(), 0, &meta)?,
            mutation(
                NODE,
                id(1),
                0,
                &Node::Leaf {
                    keys: vec![],
                    next: None,
                },
            )?,
        ])?;
        Ok(Self {
            pager,
            meta,
            meta_rev: 1,
            height: 1,
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut pager = Pager::open(path)?;
        let (meta, meta_rev): (Meta, u64) = parse(pager.get(META, "root")?.ok_or_else(invalid)?)?;
        if meta.root == 0
            || meta.root >= meta.next
            || meta.next > MAX_NODES + 1
            || meta.count > 1024
        {
            return Err(invalid());
        }
        let mut tree = Self {
            pager,
            meta,
            meta_rev,
            height: 0,
        };
        let mut seen = BTreeSet::new();
        let mut leaves = Vec::new();
        let info = tree.verify_node(tree.meta.root, 1, &mut seen, &mut leaves)?;
        if info.count != tree.meta.count
            || seen.len() != tree.meta.next as usize - 1
            || leaves
                .iter()
                .enumerate()
                .any(|(i, (_, link))| *link != leaves.get(i + 1).map(|(id, _)| *id))
        {
            return Err(invalid());
        }
        tree.height = info.height;
        Ok(tree)
    }
    fn node(&mut self, n: u32) -> Result<(Node, u64)> {
        if n == 0 || n >= self.meta.next {
            return Err(invalid());
        }
        parse(self.pager.get(NODE, &id(n))?.ok_or_else(invalid)?)
    }
    fn verify_node(
        &mut self,
        n: u32,
        depth: usize,
        seen: &mut BTreeSet<u32>,
        leaves: &mut Vec<(u32, Option<u32>)>,
    ) -> Result<Walk> {
        if depth > MAX_HEIGHT || !seen.insert(n) {
            return Err(invalid());
        }
        match self.node(n)?.0 {
            Node::Leaf { keys, next } => {
                if !keys_valid(&keys) || keys.is_empty() && n != self.meta.root {
                    return Err(invalid());
                }
                for k in &keys {
                    if self.pager.get(DATA, k)?.is_none() {
                        return Err(invalid());
                    }
                }
                leaves.push((n, next));
                Ok(Walk {
                    first: keys.first().cloned(),
                    last: keys.last().cloned(),
                    height: 1,
                    count: keys.len() as u32,
                })
            }
            Node::Branch { keys, children } => {
                if keys.is_empty() || !keys_valid(&keys) || children.len() != keys.len() + 1 {
                    return Err(invalid());
                }
                let mut out = Walk {
                    first: None,
                    last: None,
                    height: 0,
                    count: 0,
                };
                for (i, child) in children.into_iter().enumerate() {
                    let info = self.verify_node(child, depth + 1, seen, leaves)?;
                    if info.first.is_none() || info.last.is_none() {
                        return Err(invalid());
                    }
                    if i == 0 {
                        out.first = info.first.clone();
                        out.height = info.height + 1;
                    } else if info.first.as_ref() != Some(&keys[i - 1])
                        || out.last.as_ref() >= info.first.as_ref()
                        || out.height != info.height + 1
                    {
                        return Err(invalid());
                    }
                    out.last = info.last;
                    out.count += info.count;
                }
                Ok(out)
            }
        }
    }
    pub fn report(&self) -> TreeReport {
        TreeReport {
            generation: self.pager.report().generation,
            rows: self.meta.count,
            nodes: self.meta.next - 1,
            height: self.height,
        }
    }
    fn path(&mut self, key: &str) -> Result<Vec<(u32, Node, u64)>> {
        if !valid_id(key) {
            return Err(invalid());
        }
        let mut n = self.meta.root;
        let mut path = vec![];
        for _ in 0..MAX_HEIGHT {
            let (node, rev) = self.node(n)?;
            let next = match &node {
                Node::Leaf { .. } => None,
                Node::Branch { keys, children } => {
                    Some(children[keys.partition_point(|k| k.as_str() <= key)])
                }
            };
            path.push((n, node, rev));
            match next {
                Some(id) => n = id,
                None => return Ok(path),
            }
        }
        Err(invalid())
    }
    pub fn get(&mut self, key: &str) -> Result<Option<NativeEntry>> {
        let path = self.path(key)?;
        if let Node::Leaf { keys, .. } = &path.last().ok_or_else(invalid)?.1 {
            if keys.binary_search_by(|k| k.as_str().cmp(key)).is_ok() {
                return self.pager.get(DATA, key)?.ok_or_else(invalid).map(Some);
            }
        }
        Ok(None)
    }
    pub fn range(&mut self, start: &str, end: &str, limit: usize) -> Result<Vec<NativeEntry>> {
        if !valid_id(start) || !valid_id(end) || start >= end || !(1..=256).contains(&limit) {
            return Err(invalid());
        }
        let mut path = self.path(start)?;
        let (_, mut node, _) = path.pop().ok_or_else(invalid)?;
        let mut out = vec![];
        for _ in 0..self.meta.next {
            match node {
                Node::Leaf { keys, next } => {
                    for key in keys {
                        if key.as_str() >= end {
                            return Ok(out);
                        }
                        if key.as_str() >= start {
                            out.push(self.pager.get(DATA, &key)?.ok_or_else(invalid)?);
                            if out.len() == limit {
                                return Ok(out);
                            }
                        }
                    }
                    if let Some(n) = next {
                        node = self.node(n)?.0;
                    } else {
                        return Ok(out);
                    }
                }
                _ => return Err(invalid()),
            }
        }
        Err(invalid())
    }
    fn allocate(meta: &mut Meta) -> Result<u32> {
        if meta.next > MAX_NODES {
            return Err(Error::Capacity("tree: node budget"));
        }
        let n = meta.next;
        meta.next += 1;
        Ok(n)
    }
    pub fn put(&mut self, key: &str, expected: u64, value: Option<String>) -> Result<Report> {
        if value
            .as_ref()
            .is_some_and(|v| v.len() > 3000 || v.contains('\0'))
        {
            return Err(Error::Capacity("tree: value budget"));
        }
        let mut path = self.path(key)?;
        let (leaf_id, leaf, leaf_rev) = path.pop().ok_or_else(invalid)?;
        let Node::Leaf { mut keys, next } = leaf else {
            return Err(invalid());
        };
        let found = keys.binary_search_by(|k| k.as_str().cmp(key));
        let data = Change {
            namespace: DATA.into(),
            key: key.into(),
            expected_revision: expected,
            value,
        };
        if found.is_ok() {
            return self.pager.commit(&[data]);
        }
        if expected != 0 || data.value.is_none() {
            return Err(Error::Conflict("tree: missing key".into()));
        }
        let mut meta = self.meta.clone();
        meta.count += 1;
        let mut changes = vec![data];
        keys.insert(found.unwrap_err(), key.into());
        let mut split = None;
        if keys.len() > ORDER {
            let right = keys.split_off(keys.len() / 2);
            let new = Self::allocate(&mut meta)?;
            split = Some((right[0].clone(), new));
            changes.push(mutation(
                NODE,
                id(new),
                0,
                &Node::Leaf { keys: right, next },
            )?);
            changes.push(mutation(
                NODE,
                id(leaf_id),
                leaf_rev,
                &Node::Leaf {
                    keys,
                    next: Some(new),
                },
            )?);
        } else {
            changes.push(mutation(
                NODE,
                id(leaf_id),
                leaf_rev,
                &Node::Leaf { keys, next },
            )?);
        }
        let mut height = self.height;
        while let Some((separator, right)) = split.take() {
            if let Some((parent, node, rev)) = path.pop() {
                let Node::Branch {
                    mut keys,
                    mut children,
                } = node
                else {
                    return Err(invalid());
                };
                let i = keys.partition_point(|k| k < &separator);
                keys.insert(i, separator);
                children.insert(i + 1, right);
                if keys.len() > ORDER {
                    let mid = keys.len() / 2;
                    let right_keys = keys.split_off(mid + 1);
                    let promoted = keys.pop().ok_or_else(invalid)?;
                    let right_children = children.split_off(mid + 1);
                    let new = Self::allocate(&mut meta)?;
                    changes.push(mutation(
                        NODE,
                        id(new),
                        0,
                        &Node::Branch {
                            keys: right_keys,
                            children: right_children,
                        },
                    )?);
                    split = Some((promoted, new));
                }
                changes.push(mutation(
                    NODE,
                    id(parent),
                    rev,
                    &Node::Branch { keys, children },
                )?);
            } else {
                if height == MAX_HEIGHT {
                    return Err(Error::Capacity("tree: height budget"));
                }
                let root = Self::allocate(&mut meta)?;
                changes.push(mutation(
                    NODE,
                    id(root),
                    0,
                    &Node::Branch {
                        keys: vec![separator],
                        children: vec![meta.root, right],
                    },
                )?);
                meta.root = root;
                height += 1;
            }
        }
        changes.push(mutation(META, "root".into(), self.meta_rev, &meta)?);
        let report = self.pager.commit(&changes)?;
        self.meta = meta;
        self.meta_rev += 1;
        self.height = height;
        Ok(report)
    }
}
