//! Authoring-space TRS and quaternion math. Matrices are row-major; vectors are
//! columns. All public transforms validate finite, invertible render values.
use crate::{Error, Result};

pub type Vector3 = [f64; 3];
pub type Quaternion = [f64; 4];
pub type Matrix4 = [[f64; 4]; 4];
pub const IDENTITY_MATRIX: Matrix4 = [[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.],[0.,0.,0.,1.]];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vector3,
    pub rotation: Quaternion,
    pub scale: Vector3,
}
impl Default for Transform {
    fn default() -> Self { Self { translation:[0.;3], rotation:[0.,0.,0.,1.], scale:[1.;3] } }
}
impl Transform {
    pub fn validate(&self) -> Result<()> {
        if self.translation.iter().chain(&self.rotation).chain(&self.scale)
            .any(|v| !v.is_finite() || !(*v as f32).is_finite()) {
            return Err(Error::Invalid("transform exceeds finite render range"));
        }
        if (self.rotation.iter().map(|v|v*v).sum::<f64>()-1.).abs()>1e-6 {
            return Err(Error::Invalid("transform rotation must be a unit XYZW quaternion"));
        }
        if self.scale.iter().any(|v|v.abs()<1e-8) { return Err(Error::Invalid("singular transform scale")); }
        Ok(())
    }
    pub fn matrix(&self) -> Result<Matrix4> {
        self.validate()?;
        let [x,y,z,w]=self.rotation;
        let mut m=IDENTITY_MATRIX;
        let r=[[1.-2.*(y*y+z*z),2.*(x*y-z*w),2.*(x*z+y*w)],
            [2.*(x*y+z*w),1.-2.*(x*x+z*z),2.*(y*z-x*w)],
            [2.*(x*z-y*w),2.*(y*z+x*w),1.-2.*(x*x+y*y)]];
        for i in 0..3 { for j in 0..3 {m[i][j]=r[i][j]*self.scale[j];} m[i][3]=self.translation[i]; }
        Ok(m)
    }
    pub fn interpolate(self, other:Self, t:f64) -> Self {
        Self { translation:lerp(self.translation,other.translation,t),
            rotation:quat_slerp(self.rotation,other.rotation,t), scale:lerp(self.scale,other.scale,t) }
    }
}
pub fn add(a:Vector3,b:Vector3)->Vector3 {std::array::from_fn(|i|a[i]+b[i])}
pub fn sub(a:Vector3,b:Vector3)->Vector3 {std::array::from_fn(|i|a[i]-b[i])}
pub fn mul(a:Vector3,s:f64)->Vector3 {a.map(|v|v*s)}
pub fn dot(a:Vector3,b:Vector3)->f64 {a.iter().zip(b).map(|(a,b)|a*b).sum()}
pub fn cross(a:Vector3,b:Vector3)->Vector3 {[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}
pub fn length(a:Vector3)->f64 {a[0].hypot(a[1]).hypot(a[2])}
pub fn normalized(a:Vector3)->Result<Vector3> {
    let n=length(a); if !n.is_finite() || n<1e-12 {Err(Error::Invalid("zero or non-finite direction"))} else {Ok(mul(a,1./n))}
}
pub fn lerp(a:Vector3,b:Vector3,t:f64)->Vector3 {std::array::from_fn(|i|a[i]+(b[i]-a[i])*t)}
pub fn matrix_mul(a:Matrix4,b:Matrix4)->Matrix4 {
    std::array::from_fn(|i|std::array::from_fn(|j|(0..4).map(|k|a[i][k]*b[k][j]).sum()))
}
pub fn transform_point(m:Matrix4,p:Vector3)->Vector3 {
    std::array::from_fn(|i|m[i][0]*p[0]+m[i][1]*p[1]+m[i][2]*p[2]+m[i][3])
}
pub fn transform_vector(m:Matrix4,p:Vector3)->Vector3 {
    std::array::from_fn(|i|m[i][0]*p[0]+m[i][1]*p[1]+m[i][2]*p[2])
}
pub fn inverse(m:Matrix4)->Result<Matrix4> {
    if m.iter().flatten().any(|v|!v.is_finite()) {return Err(Error::Invalid("non-finite matrix"));}
    let mut a=[[0.;8];4];
    for i in 0..4 {a[i][..4].copy_from_slice(&m[i]);a[i][i+4]=1.;}
    for col in 0..4 {
        let pivot=(col..4).max_by(|&i,&j|a[i][col].abs().total_cmp(&a[j][col].abs())).unwrap();
        if a[pivot][col].abs()<1e-14 {return Err(Error::Invalid("singular matrix"));}
        a.swap(col,pivot);let factor=a[col][col];for v in &mut a[col] {*v/=factor;}
        for i in 0..4 {if i!=col {let f=a[i][col];for j in 0..8 {a[i][j]-=f*a[col][j];}}}
    }
    let out:Matrix4=std::array::from_fn(|i|std::array::from_fn(|j|a[i][j+4]));
    if out.iter().flatten().any(|v|!v.is_finite()) {return Err(Error::Invalid("inverse exceeds numeric range"));}
    Ok(out)
}
pub fn quat_mul(a:Quaternion,b:Quaternion)->Quaternion {
    [a[3]*b[0]+a[0]*b[3]+a[1]*b[2]-a[2]*b[1],
     a[3]*b[1]-a[0]*b[2]+a[1]*b[3]+a[2]*b[0],
     a[3]*b[2]+a[0]*b[1]-a[1]*b[0]+a[2]*b[3],
     a[3]*b[3]-a[0]*b[0]-a[1]*b[1]-a[2]*b[2]]
}
pub fn quat_inverse(q:Quaternion)->Quaternion {[-q[0],-q[1],-q[2],q[3]]}
pub fn quat_normalize(q:Quaternion)->Result<Quaternion> {
    let n=q.iter().map(|v|v*v).sum::<f64>().sqrt();
    if !n.is_finite()||n<1e-12 {return Err(Error::Invalid("zero quaternion"));} Ok(q.map(|v|v/n))
}
pub fn quat_axis_angle(axis:Vector3,angle:f64)->Result<Quaternion> {
    if !angle.is_finite() {return Err(Error::Invalid("non-finite angle"));}
    let a=normalized(axis)?; let (s,c)=(angle*0.5).sin_cos();Ok([a[0]*s,a[1]*s,a[2]*s,c])
}
pub fn quat_rotate(q:Quaternion,p:Vector3)->Vector3 {
    let u=[q[0],q[1],q[2]];add(p,mul(add(mul(cross(u,p),q[3]),cross(u,cross(u,p))),2.))
}
pub fn quat_from_to(from:Vector3,to:Vector3)->Result<Quaternion> {
    let a=normalized(from)?;let b=normalized(to)?;let d=dot(a,b).clamp(-1.,1.);
    if d>1.-1e-12 {return Ok([0.,0.,0.,1.]);}
    if d< -1.+1e-12 {
        let axis=if a[0].abs()<0.8 {[1.,0.,0.]}else{[0.,1.,0.]};
        return quat_axis_angle(cross(a,axis),std::f64::consts::PI);
    }
    let c=cross(a,b);quat_normalize([c[0],c[1],c[2],1.+d])
}
pub fn quat_slerp(a:Quaternion,mut b:Quaternion,t:f64)->Quaternion {
    let mut d=a.iter().zip(b).map(|(a,b)|a*b).sum::<f64>();
    if d<0. {b=b.map(|v|-v);d=-d;}
    if d>0.9995 {return quat_normalize(std::array::from_fn(|i|a[i]+t*(b[i]-a[i]))).unwrap_or(a);}
    let theta=d.clamp(-1.,1.).acos();let denom=theta.sin();
    std::array::from_fn(|i|(((1.-t)*theta).sin()*a[i]+(t*theta).sin()*b[i])/denom)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn rotated_scaled_parent_and_inverse_agree() {
        let t=Transform{translation:[3.,-2.,7.],rotation:quat_axis_angle([0.,1.,0.],0.7).unwrap(),scale:[2.,3.,0.5]};
        let m=t.matrix().unwrap();let p=[1.,4.,-3.];let back=transform_point(inverse(m).unwrap(),transform_point(m,p));
        assert!(length(sub(back,p))<1e-12);
        let q=quat_from_to([0.,0.,-1.],[1.,0.,0.]).unwrap();
        assert!(length(sub(quat_rotate(q,[0.,0.,-1.]),[1.,0.,0.]))<1e-12);
    }
}
