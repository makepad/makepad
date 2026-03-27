use crate::{MpDocument, MpDocumentId, MpResourceUpdate, MpScene};

#[derive(Clone, Debug)]
pub struct MpTransaction {
    pub document_id: MpDocumentId,
    pub ops: Vec<MpTransactionOp>,
    pub generate_frame: bool,
}

#[derive(Clone, Debug)]
pub enum MpTransactionOp {
    ReplaceScene(MpScene),
    UpdateResources(Vec<MpResourceUpdate>),
}

impl MpTransaction {
    pub fn new(document_id: MpDocumentId) -> Self {
        Self {
            document_id,
            ops: Vec::new(),
            generate_frame: true,
        }
    }

    pub fn replace_scene(mut self, scene: MpScene) -> Self {
        self.ops.push(MpTransactionOp::ReplaceScene(scene));
        self
    }

    pub fn update_resources(mut self, updates: Vec<MpResourceUpdate>) -> Self {
        self.ops.push(MpTransactionOp::UpdateResources(updates));
        self
    }

    pub fn apply(self, document: &mut MpDocument) {
        debug_assert_eq!(document.id, self.document_id);
        for op in self.ops {
            match op {
                MpTransactionOp::ReplaceScene(scene) => {
                    document.scene = scene;
                }
                MpTransactionOp::UpdateResources(updates) => {
                    for update in updates {
                        match update {
                            MpResourceUpdate::UpsertGlyphRun { key, glyph_run } => {
                                document.glyph_runs.insert(key, glyph_run);
                            }
                            MpResourceUpdate::DeleteGlyphRun(key) => {
                                document.glyph_runs.remove(&key);
                            }
                            // Fonts, images, and external images are renderer-scoped after
                            // Phase 4. `MpDocument` no longer owns them, so document-local
                            // transaction apply intentionally ignores those updates here.
                            MpResourceUpdate::UpsertImage { .. }
                            | MpResourceUpdate::DeleteImage(_)
                            | MpResourceUpdate::UpsertFont { .. }
                            | MpResourceUpdate::DeleteFont(_)
                            | MpResourceUpdate::UpsertExternalImage { .. }
                            | MpResourceUpdate::DeleteExternalImage(_) => {}
                        }
                    }
                }
            }
        }
        if self.generate_frame {
            document.epoch = document.epoch.saturating_add(1);
        }
    }
}
