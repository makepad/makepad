//! MultibodyJoints using the reduced-coordinates formalism or using constraints.

pub use self::multibody::Multibody;
pub use self::multibody_ik::InverseKinematicsOption;
pub use self::multibody_joint::MultibodyJoint;
pub use self::multibody_joint_set::{
    MultibodyIndex, MultibodyJointHandle, MultibodyJointSet, MultibodyLinkId,
};
pub use self::multibody_link::MultibodyLink;
pub use self::unit_multibody_joint::{unit_joint_limit_constraint, unit_joint_motor_constraint};

mod multibody;
mod multibody_joint_set;
mod multibody_link;
mod multibody_workspace;

mod multibody_ik;
mod multibody_joint;
mod unit_multibody_joint;
