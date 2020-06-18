use crate::ident::Ident;
use crate::ty::Ty;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Builtin {
    pub return_tys: HashMap<Vec<Ty>, Ty>,
}

pub fn generate_builtins() -> HashMap<Ident, Builtin> {
    [
        (
            Ident::new("cos"),
            Builtin {
                return_tys: [
                    (vec![Ty::Float], Ty::Float),
                    (vec![Ty::Vec2], Ty::Vec2),
                    (vec![Ty::Vec3], Ty::Vec3),
                    (vec![Ty::Vec4], Ty::Vec4),
                ]
                .iter()
                .cloned()
                .collect(),
            },
        ),
        (
            Ident::new("max"),
            Builtin {
                return_tys: [
                    (vec![Ty::Float, Ty::Float], Ty::Float),
                    (vec![Ty::Vec2, Ty::Vec2], Ty::Vec2),
                    (vec![Ty::Vec3, Ty::Vec2], Ty::Vec3),
                    (vec![Ty::Vec4, Ty::Vec4], Ty::Vec4),
                ]
                .iter()
                .cloned()
                .collect(),
            },
        ),
        (
            Ident::new("min"),
            Builtin {
                return_tys: [
                    (vec![Ty::Float, Ty::Float], Ty::Float),
                    (vec![Ty::Vec2, Ty::Vec2], Ty::Vec2),
                    (vec![Ty::Vec3, Ty::Vec2], Ty::Vec3),
                    (vec![Ty::Vec4, Ty::Vec4], Ty::Vec4),
                ]
                .iter()
                .cloned()
                .collect(),
            },
        ),
        (
            Ident::new("sin"),
            Builtin {
                return_tys: [
                    (vec![Ty::Float], Ty::Float),
                    (vec![Ty::Vec2], Ty::Vec2),
                    (vec![Ty::Vec3], Ty::Vec3),
                    (vec![Ty::Vec4], Ty::Vec4),
                ]
                .iter()
                .cloned()
                .collect(),
            },
        ),
        (
            Ident::new("tan"),
            Builtin {
                return_tys: [
                    (vec![Ty::Float], Ty::Float),
                    (vec![Ty::Vec2], Ty::Vec2),
                    (vec![Ty::Vec3], Ty::Vec3),
                    (vec![Ty::Vec4], Ty::Vec4),
                ]
                .iter()
                .cloned()
                .collect(),
            },
        ),
    ]
    .iter()
    .cloned()
    .collect()
}
