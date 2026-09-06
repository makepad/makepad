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
        Ok(json::obj(vec![("head",crate::head_json(self.head())),("valid",Value::Bool(errors.is_empty())),
            ("closed_objects",Value::Int(closed as i64)),("open_objects",Value::Int(open as i64)),("errors",Value::Arr(errors))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine,Limits,head_json};
    #[test]
    fn inward_closed_shells_fail_review_and_flip_repairs_them(){
        let mut engine=Engine::new(Limits::default());engine.open("car",None,None).unwrap();
        let mut apply=|engine:&mut Engine,id:&str,operations:Value|{
            let expected=head_json(engine.document("car").unwrap().head());
            engine.execute("model.apply",&json::obj(vec![("document",json::s("car")),("request_id",json::s(id)),("expected",expected),("operations",operations)]),None).unwrap()
        };
        apply(&mut engine,"box",json::parse(br#"[{"op":"cube","object":"body","size":[2,1,4]}]"#).unwrap());
        assert_eq!(engine.document("car").unwrap().publication_checks(None).unwrap().get("valid"),Some(&Value::Bool(true)));
        let faces=engine.document("car").unwrap().object("body").unwrap().faces().iter().map(|f|json::s(f.id.0.to_string())).collect::<Vec<_>>();
        let flip=Value::Arr(vec![json::obj(vec![("op",json::s("flip_faces")),("object",json::s("body")),("faces",Value::Arr(faces))])]);
        apply(&mut engine,"invert",flip.clone());
        let broken=engine.document("car").unwrap().publication_checks(None).unwrap();assert_eq!(broken.get("valid"),Some(&Value::Bool(false)));assert!(broken.to_json().contains("orientation_errors"));
        apply(&mut engine,"repair",flip);
        assert_eq!(engine.document("car").unwrap().publication_checks(None).unwrap().get("valid"),Some(&Value::Bool(true)));
    }
    #[test]
    fn summary_mode_preserves_exact_commit_and_replay_without_id_chatter(){
        let mut engine=Engine::new(Limits::default());engine.open("car",None,None).unwrap();
        let args=json::obj(vec![("document",json::s("car")),("request_id",json::s("box")),("expected",head_json(engine.document("car").unwrap().head())),
            ("result_mode",json::s("summary")),("operations",json::parse(br#"[{"op":"cylinder","object":"tire","radius":1,"height":1,"segments":64}]"#).unwrap())]);
        let result=engine.execute("model.apply",&args,None).unwrap();assert!(result.to_json().len()<400);assert!(result.get("results").is_none());
        let replay=engine.execute("model.apply",&args,None).unwrap();assert_eq!(result.get("head"),replay.get("head"));assert_eq!(replay.get("replayed"),Some(&Value::Bool(true)));
    }
}
