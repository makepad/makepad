//! Bounded real-solver review scenarios, executed on the document worker.
use crate::{*,transform::*};
use makepad_game_sim::soft_body::*;
use std::sync::Arc;

pub(crate) fn sample(doc:&Document,scenario:&str,cancelled:Option<&dyn Fn()->bool>)->Result<(Vec<Matrix4>,Vec<Matrix4>,SoftBodyStats)> {
    let metadata=doc.soft_body().ok_or(Error::Invalid("soft-body review requires soft_body_bind"))?;
    let skeleton=doc.skeleton().ok_or(Error::Invalid("soft-body review requires skeleton"))?;
    let definition=Arc::new(crate::soft_body::definition(metadata));
    let settings=crate::soft_body::core_settings(metadata.settings);
    let mut state=SoftBodyState::new(definition.clone(),settings,SoftBodyPose::default()).map_err(|_|Error::Invalid("soft-body review initialization"))?;
    let body=doc.object(&metadata.object).ok_or(Error::Invalid("soft-body review object"))?;
    let frame=doc.scene().world_matrix(&metadata.object)?;
    let mut low=[f64::INFINITY;3];let mut high=[f64::NEG_INFINITY;3];
    for vertex in body.vertices(){let p=transform_point(frame,vertex.position);for i in 0..3{low[i]=low[i].min(p[i]);high[i]=high[i].max(p[i]);}}
    let radius=((high[0]-low[0]).max(high[1]-low[1]).max(high[2]-low[2])*0.5)as f32;
    let mut last=SoftBodyStats::default();
    for tick in 0..90 {
        if cancelled.is_some_and(|cancel|cancel()){return Err(Error::Invalid("soft-body review cancelled"));}
        let mut pose=SoftBodyPose::default();let mut contacts=Vec::new();
        match scenario {
            "acceleration"=>{if tick>=75 {let t=(tick-75)as f32/60.;pose.translation[0]=radius*8.*t*t;}},
            "landing"=>{
                let t=(tick as f32/60.-1.).max(0.);
                pose.translation[1]=if tick<60{radius*0.7}else{(radius*0.7-4.9*t*t).max(0.)};
                contacts.push(SoftBodyCollider::Plane{normal:[0.,1.,0.],offset:low[1]as f32});
            },
            "wall"=>{
                let compress=((tick as f32-55.)/35.).clamp(0.,1.);
                contacts.push(SoftBodyCollider::Plane{normal:[-1.,0.,0.],offset:-(high[0]as f32-radius*0.18*compress)});
            },
            _=>return Err(Error::Invalid("soft-body scenario must be acceleration, landing or wall")),
        }
        let stats=state.step(1./60.,pose,&contacts).map_err(|_|Error::Invalid("soft-body review step"))?;
        if stats.recovered{return Err(Error::Invalid("soft-body review recovered invalid cells; tune binding/settings"));}
        last=stats;
    }
    let mut output=SoftBodyFrame::default();state.write_frame(&mut output);
    let rest=doc.rig().global_rest(skeleton)?;let mut joints=rest.clone();
    for (&joint,affine) in metadata.tet_joints.iter().zip(&output.tetrahedra) {
        let mut matrix=IDENTITY_MATRIX;for row in 0..3{for col in 0..4{matrix[row][col]=affine.matrix[row][col]as f64;}}
        joints[joint as usize]=matrix_mul(matrix,rest[joint as usize]);
    }
    for attachment in &metadata.attachments {
        let binding=SoftBodyBinding{tetrahedron:attachment.binding.tetrahedron,weights:attachment.binding.weights};
        let rigid=output.attachment(&definition,binding).map_err(|_|Error::Invalid("soft-body rigid review frame"))?;
        joints[attachment.joint as usize]=Transform{translation:rigid.position.map(f64::from),rotation:rigid.rotation.map(f64::from),scale:[1.;3]}.matrix()?;
    }
    // Rest local child controls follow the rigid eye/limb attachment, exactly
    // as runtime's hierarchical palette override. No cage shear enters eyes.
    for (i,joint) in skeleton.joints.iter().enumerate(){
        if metadata.tet_joints.contains(&(i as u16)) || metadata.attachments.iter().any(|a|a.joint as usize==i){continue;}
        if let Some(parent)=joint.parent{joints[i]=matrix_mul(joints[parent as usize],doc.rig().local_rest(skeleton,i)?.matrix()?);}
    }
    let palette=joints.iter().zip(rest).map(|(posed,rest)|Ok(matrix_mul(*posed,inverse(rest)?))).collect::<Result<Vec<_>>>()?;
    Ok((palette,joints,last))
}
