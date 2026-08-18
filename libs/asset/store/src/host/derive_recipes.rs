//! Stock derived-variant recipes this server advertises.
//!
//! Games request these exact canonical documents. A missing or pending
//! variant is a refusal — never a silent original-file substitute.

use super::json::{obj, s, Value};
use super::util::to_hex;
use makepad_asset_data::{
    FileRole, ProcessingRecipe, RecipeSettings, ThumbnailMedia, ToolClosure, OUTPUT_SCHEMA_V1,
};

/// Stable tool identity pinned into every advertised recipe digest.
pub fn stock_tool() -> ToolClosure {
    ToolClosure {
        processor: "makepad_derive".into(),
        version: "1".into(),
        build: "v1".into(),
        deterministic: true,
    }
}

pub fn stock_image_resize_256() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::ImageResize {
            source_role: FileRole::Texture,
            width: 256,
            height: 256,
            media: ThumbnailMedia::Png,
        },
        tool: stock_tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

pub fn stock_mesh_ao_256() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::MeshAo { resolution: 256 },
        tool: stock_tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

pub struct StockRecipe {
    pub id: &'static str,
    pub kind: &'static str,
    pub label: &'static str,
    pub recipe: ProcessingRecipe,
}

pub fn stock_recipes() -> Vec<StockRecipe> {
    vec![
        StockRecipe {
            id: "image_resize_256_png",
            kind: "derive.image_resize",
            label: "Texture resize 256×256 PNG",
            recipe: stock_image_resize_256(),
        },
        StockRecipe {
            id: "mesh_ao_256",
            kind: "derive.mesh_ao",
            label: "Mesh AO bake 256 (aomesh)",
            recipe: stock_mesh_ao_256(),
        },
    ]
}

pub fn recipes_json() -> Value {
    let rows: Vec<Value> = stock_recipes()
        .into_iter()
        .map(|r| {
            let bytes = r.recipe.to_canonical_bytes().expect("stock recipe canonical");
            obj(vec![
                ("id", s(r.id)),
                ("kind", s(r.kind)),
                ("label", s(r.label)),
                ("recipe", s(to_hex(&bytes))),
                ("recipe_digest", s(r.recipe.digest().expect("stock recipe digest").to_string())),
            ])
        })
        .collect();
    obj(vec![("recipes", Value::Arr(rows))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::RecipeKind;

    #[test]
    fn stock_recipes_are_canonical_and_deterministic() {
        let a = recipes_json().to_json();
        let b = recipes_json().to_json();
        assert_eq!(a, b);
        let recipes = stock_recipes();
        assert_eq!(recipes.len(), 2);
        assert_eq!(recipes[0].recipe.kind(), RecipeKind::ImageResize);
        assert_eq!(recipes[1].recipe.kind(), RecipeKind::MeshAo);
        assert!(recipes[0].recipe.to_canonical_bytes().unwrap().len() > 8);
    }
}
