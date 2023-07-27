use crate::{makepad_draw::*, makepad_image_formats::*};

pub trait ImageLoadingWidget {
    fn image_filename(&self) -> &LiveDependency;
    fn get_texture(&self) -> &Option<Texture>;
    fn set_texture(&mut self, texture: Option<Texture>);

    fn after_apply_for_image_loading_widget(
        &mut self,
        cx: &mut Cx,
        _from: ApplyFrom,
        index: usize,
        nodes: &[LiveNode],
    ) {
        if self.get_texture().is_none() {
            self.create_texture_from_image(cx, index, nodes);
        }
    }

    fn create_texture_from_image(&mut self, cx: &mut Cx, index: usize, nodes: &[LiveNode]) {
        let binding = self.image_filename().clone();
        let image_path = binding.as_str();

        if image_path.len() > 0 {
            if let Some(cached_texture) = cx.image_cache.get(image_path) {
                self.set_texture(Some(cached_texture.clone()));
            } else {
                let texture = Texture::new(cx);

                if let Some(mut buffer) = Self::load_image_dependency(cx, image_path, index, nodes)
                {
                    texture.set_desc(
                        cx,
                        TextureDesc {
                            format: TextureFormat::ImageBGRA,
                            width: Some(buffer.width),
                            height: Some(buffer.height),
                        },
                    );
                    texture.swap_image_u32(cx, &mut buffer.data);
                }

                self.set_texture(Some(texture.clone()));

                cx.image_cache.put(image_path, texture);
            }
        }
    }

    fn load_image_dependency(
        cx: &mut Cx,
        image_path: &str,
        index: usize,
        nodes: &[LiveNode],
    ) -> Option<ImageBuffer> {
        match cx.get_dependency(image_path) {
            Ok(data) => {
                if image_path.ends_with(".jpg") {
                    match jpeg::decode(data) {
                        Ok(image) => Some(image),
                        Err(err) => {
                            cx.apply_image_decoding_failed(
                                live_error_origin!(),
                                index,
                                nodes,
                                image_path,
                                &err,
                            );
                            None
                        }
                    }
                } else if image_path.ends_with(".png") {
                    match png::decode(data) {
                        Ok(image) => Some(image),
                        Err(err) => {
                            cx.apply_image_decoding_failed(
                                live_error_origin!(),
                                index,
                                nodes,
                                image_path,
                                &err,
                            );
                            None
                        }
                    }
                } else {
                    cx.apply_image_type_not_supported(
                        live_error_origin!(),
                        index,
                        nodes,
                        image_path,
                    );
                    None
                }
            }
            Err(err) => {
                cx.apply_resource_not_loaded(live_error_origin!(), index, nodes, image_path, &err);
                None
            }
        }
    }
}
