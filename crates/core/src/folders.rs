use super::*;

impl Store {
    /// Independent handle for background tree writes; begin_write reloads under the library lock.
    pub fn tree_writer(&self) -> Self {
        Self { root: self.root.clone(), index: self.index.clone(), trash: HashMap::new(),
            loaded: Default::default(), parsed: HashMap::new(), warnings: vec![] }
    }

    pub(super) fn migrate_folders(&mut self) -> Result<()> {
        let _lock = self.begin_write()?;
        let legacy: Vec<String> = self.index.values().filter_map(|m| m.parent.clone())
            .filter(|id| self.index.get(id).is_some_and(|m| m.folder.is_none()) && self.path(id).is_file())
            .collect::<std::collections::HashSet<_>>().into_iter().collect();
        if !legacy.is_empty() {
            let backup = self.root.join("index.before-folders.json");
            if !backup.exists() { write_atomic(&backup, &serde_json::to_vec_pretty(&self.index)?)?; }
            let previous = self.index.clone();
            let mapping: HashMap<String, String> = legacy.iter().map(|id| (id.clone(), uuid::Uuid::new_v4().to_string())).collect();
            for id in &legacy {
                let doc = self.load(id)?;
                let mut meta = self.index[id].clone();
                meta.folder = Some(doc.title.text.clone());
                meta.pinned = false;
                meta.starred = 0;
                meta.parent = meta.parent.map(|p| mapping.get(&p).cloned().unwrap_or(p));
                self.index.insert(mapping[id].clone(), meta);
            }
            for (id, meta) in &mut self.index {
                if meta.folder.is_some() { continue; }
                if let Some(folder) = mapping.get(id) { meta.parent = Some(folder.clone()); meta.order = 1; }
                else if let Some(folder) = meta.parent.as_ref().and_then(|p| mapping.get(p)) { meta.parent = Some(folder.clone()); }
            }
            if let Err(e) = self.save_index() { self.index = previous; return Err(e); }
        }
        write_atomic(&self.root.join("format.json"), &serde_json::to_vec_pretty(&FormatMarker { format: FORMAT })?)?;
        Ok(())
    }

    fn validate_folder(&self, id: Option<&str>, parent: Option<&str>) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        let mut p = parent;
        while let Some(key) = p {
            anyhow::ensure!(Some(key) != id && seen.insert(key), "不能移动到自身或后代文件夹中");
            let m = self.index.get(key).filter(|m| m.folder.is_some()).ok_or_else(|| anyhow::anyhow!("目标文件夹不存在"))?;
            p = m.parent.as_deref();
        }
        Ok(())
    }

    pub fn create_folder(&mut self, title: &str, parent: Option<&str>) -> Result<String> {
        let _lock = self.begin_write()?;
        self.validate_folder(None, parent)?;
        let id = uuid::Uuid::new_v4().to_string();
        let t = now();
        self.index.insert(id.clone(), Meta { folder: Some(title.to_string()), parent: parent.map(str::to_string), created: t, modified: t, ..Default::default() });
        if let Err(e) = self.save_index() { self.index.remove(&id); return Err(e); }
        Ok(id)
    }

    /// Persist one drop atomically; neither document content nor timestamps change.
    pub fn move_in_tree(&mut self, id: &str, parent: Option<&str>, before: Option<&str>) -> Result<()> {
        let _lock = self.begin_write()?;
        anyhow::ensure!(self.index_has(id), "文档或文件夹不存在");
        self.validate_folder(Some(id), parent)?;
        if let Some(anchor) = before {
            anyhow::ensure!(self.index_has(anchor) && self.index[anchor].parent.as_deref() == parent, "目标位置已改变，请重试");
            if anchor == id { return Ok(()); }
        }
        let mut siblings: Vec<String> = self.index.iter().filter(|(k, m)| self.index_has(k) && m.parent.as_deref() == parent).map(|(k, _)| k.clone()).collect();
        siblings.sort_by(|a, b| {
            let (a, b) = (&self.index[a], &self.index[b]);
            if parent.is_some() { a.sibling_key().cmp(&b.sibling_key()) }
            else { (a.root_order.unwrap_or(0), std::cmp::Reverse(a.modified)).cmp(&(b.root_order.unwrap_or(0), std::cmp::Reverse(b.modified))) }
        });
        siblings.retain(|key| key != id);
        let pos = before.and_then(|key| siblings.iter().position(|x| x == key)).unwrap_or(siblings.len());
        siblings.insert(pos, id.to_string());
        let previous = self.index.clone();
        self.index.get_mut(id).unwrap().parent = parent.map(str::to_string);
        for (i, key) in siblings.iter().enumerate() {
            let m = self.index.get_mut(key).unwrap();
            if parent.is_none() { m.root_order = Some(i as u64 + 1); }
            else { m.order = i as u64 + 1; m.root_order = None; }
        }
        if let Err(e) = self.save_index() { self.index = previous; return Err(e); }
        Ok(())
    }
}
