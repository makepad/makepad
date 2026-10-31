//! Objective publication checks, independent of any language model judgement.
use crate::{Document, Result, mesh, json::{self,Value}};
impl Document {
    pub fn publication_checks(&self,cancelled:Option<&dyn Fn()->bool>)->Result<Value>{
        let mut ctx=mesh::Context::new(self.limits().mesh.clone(),cancelled);
        let evaluated=self.scene().evaluated_meshes(&self.state,&mut ctx)?;
        let mut errors=Vec::new();let mut open=0usize;let mut closed=0usize;
        for (name,model) in evaluated {
            ctx.checkpoint(1)?;
            let local=model.validate(&mut ctx)?;
            if !local.is_valid_surface {
                errors.push(json::obj(vec![("object",json::s(&name)),("reason",json::s("invalid topology or inconsistent face orientation"))]));
                continue;
            }
            // Sheets (windows, decals, leaves) need no closed-solid certificate.
            if !local.is_closed_manifold{open+=1;continue;}
            closed+=1;
            let report=model.validate_global(&mut ctx)?;
            if report.orientation_errors>0 || report.intersection_count>0 {
                errors.push(json::obj(vec![("object",json::s(&name)),("orientation_errors",Value::Int(report.orientation_errors as i64)),
                    ("intersections",Value::Int(report.intersection_count as i64))]));
            }
        }
        Ok(json::obj(vec![("head",crate::service::head_json(self.head())),("valid",Value::Bool(errors.is_empty())),
            ("closed_objects",Value::Int(closed as i64)),("open_objects",Value::Int(open as i64)),("errors",Value::Arr(errors))]))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, Limits, Operation, Transaction, MeshEditingOperation, Action, mesh};
    #[test]
    fn inward_closed_shells_fail_review_and_flip_repairs_them() {
        let mut doc=Document::new(Limits::default()).unwrap();
        let apply=|doc:&mut Document,id:&str,operations:Vec<Operation>| {
            doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None).unwrap()
        };
        apply(&mut doc,"box",vec![Operation::Cube{object:"body".into(),size:[2.,1.,4.]}]);
        let review=|doc:&Document| doc.object("body").unwrap().validate_global(&mut mesh::Context::default()).unwrap();
        assert!(review(&doc).is_valid_solid);
        let faces=doc.object("body").unwrap().faces().iter().map(|f|f.id).collect();
        let flip=vec![Operation::MeshEditing(MeshEditingOperation{object:"body".into(),action:Action::FlipFaces{faces}})];
        apply(&mut doc,"invert",flip.clone());
        let broken=review(&doc);assert!(!broken.is_valid_solid);assert!(broken.orientation_errors>0);
        apply(&mut doc,"repair",flip);
        assert!(review(&doc).is_valid_solid);
    }
    #[test]
    fn summary_mode_preserves_exact_commit_and_replay_without_id_chatter() {
        let mut doc=Document::new(Limits::default()).unwrap();
        let args=Transaction{request_id:"box".into(),expected:doc.head(),
            operations:vec![Operation::cylinder("tire",1.,1.,64,true,doc.limits()).unwrap()]};
        let result=doc.apply(args.clone(),None).unwrap();
        let replay=doc.apply(args,None).unwrap();
        assert_eq!(result.current,replay.current);
        assert_eq!(result.committed,replay.committed);
        assert!(replay.replayed);
    }
}
