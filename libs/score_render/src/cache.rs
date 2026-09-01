use crate::{PageId, PaintDiff, PaintItem, PaintList, PaintListError, SemanticId};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
struct CachedPage {
    list: Arc<PaintList>,
    last_used_frame: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageCacheStats {
    pub pages: usize,
    pub items: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageInvalidation {
    pub page: PageId,
    pub old_revision: Option<u64>,
    pub new_revision: u64,
    /// Only these semantic instance records need replacement in live GPU buffers.
    pub diff: PaintDiff,
    /// Raster tiles include the revision in their key and therefore become stale.
    pub invalidate_raster_tiles: bool,
}

/// Retains independently replaceable page display lists.
///
/// Edits compare stable semantic IDs and report the exact instance records to
/// upload. Other pages remain shared `Arc`s and are neither rebuilt nor touched.
#[derive(Clone, Debug)]
pub struct PageCache {
    pages: BTreeMap<PageId, CachedPage>,
    max_bytes: usize,
    resident_bytes: usize,
}

impl PageCache {
    pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

    pub fn new(max_bytes: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            max_bytes,
            resident_bytes: 0,
        }
    }

    pub fn get(&mut self, page: PageId, frame: u64) -> Option<Arc<PaintList>> {
        let entry = self.pages.get_mut(&page)?;
        entry.last_used_frame = frame;
        Some(entry.list.clone())
    }

    pub fn peek(&self, page: PageId) -> Option<&Arc<PaintList>> {
        self.pages.get(&page).map(|entry| &entry.list)
    }

    pub fn insert(&mut self, list: Arc<PaintList>, frame: u64) -> PageInvalidation {
        let page = list.page_id();
        let old = self.pages.remove(&page);
        if let Some(old) = &old {
            self.resident_bytes = self.resident_bytes.saturating_sub(old.list.memory_bytes());
        }
        let diff = old
            .as_ref()
            .map_or_else(|| PaintDiff {
                added: list.items().iter().map(|item| item.id).collect(),
                ..PaintDiff::default()
            }, |old| old.list.diff(&list));
        let invalidation = PageInvalidation {
            page,
            old_revision: old.as_ref().map(|entry| entry.list.revision()),
            new_revision: list.revision(),
            invalidate_raster_tiles: !diff.is_empty(),
            diff,
        };
        self.resident_bytes += list.memory_bytes();
        self.pages.insert(
            page,
            CachedPage {
                list,
                last_used_frame: frame,
            },
        );
        self.evict_to_budget(Some(page));
        invalidation
    }

    pub fn patch(
        &mut self,
        page: PageId,
        revision: u64,
        remove: &[SemanticId],
        replacements: Vec<PaintItem>,
        frame: u64,
    ) -> Result<PageInvalidation, PaintListError> {
        let Some(old) = self.pages.get(&page).map(|entry| entry.list.clone()) else {
            return Err(PaintListError::MissingPage(page));
        };
        let new = Arc::new(old.patched(revision, remove, replacements)?);
        Ok(self.insert(new, frame))
    }

    pub fn remove(&mut self, page: PageId) -> Option<Arc<PaintList>> {
        let entry = self.pages.remove(&page)?;
        self.resident_bytes = self.resident_bytes.saturating_sub(entry.list.memory_bytes());
        Some(entry.list)
    }

    pub fn stats(&self) -> PageCacheStats {
        PageCacheStats {
            pages: self.pages.len(),
            items: self
                .pages
                .values()
                .map(|entry| entry.list.items().len())
                .sum(),
            bytes: self.resident_bytes,
        }
    }

    fn evict_to_budget(&mut self, protected: Option<PageId>) {
        while self.resident_bytes > self.max_bytes && self.pages.len() > 1 {
            let victim = self
                .pages
                .iter()
                .filter(|(page, _)| Some(**page) != protected)
                .min_by_key(|(page, entry)| (entry.last_used_frame, **page))
                .map(|(page, _)| *page);
            let Some(victim) = victim else {
                break;
            };
            self.remove(victim);
        }
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ink, InkRole, Point, Primitive, Rect, RuleKind};

    fn page(page: u32, revision: u64, x: f64) -> Arc<PaintList> {
        Arc::new(
            PaintList::new(
                PageId(page),
                revision,
                Point::new(100.0, 140.0),
                vec![PaintItem::primitive(
                    SemanticId(1),
                    0,
                    Ink::role(InkRole::Staff),
                    Primitive::Rule {
                        rect: Rect::from_xywh(x, 1.0, 10.0, 0.13),
                        kind: RuleKind::Staff,
                        staff_group: Some(1),
                    },
                )],
            )
            .unwrap(),
        )
    }

    #[test]
    fn replacing_one_page_reports_one_semantic_upload() {
        let mut cache = PageCache::default();
        cache.insert(page(0, 1, 0.0), 1);
        cache.insert(page(1, 1, 0.0), 1);
        let untouched = cache.peek(PageId(1)).unwrap().clone();
        let invalidation = cache.insert(page(0, 2, 3.0), 2);
        assert_eq!(invalidation.diff.changed, vec![SemanticId(1)]);
        assert_eq!(invalidation.diff.upload_count(), 1);
        assert!(Arc::ptr_eq(&untouched, cache.peek(PageId(1)).unwrap()));
    }
}
