//! A hosted app may render on a different physical GPU from its compositor.
//! OPAQUE_FD images and their timeline stay on the compositor's GPU; this
//! process bridges local rendering there through DMA_BUF + SYNC_FD. Every
//! pixel transfer is a GPU command. No CPU mapping or readback is involved.
use super::gpu_bridge::GpuBridge;
use super::*;
use crate::{
    os::shared_framebuf::LinuxOwnedImage,
    texture::{Texture, WeakTexture},
};
use makepad_studio_protocol::PresentableDraw;

pub(super) struct HostedRoute {
    host: Box<CxVulkan>,
    images: HashMap<VulkanTextureKey, RoutedImage>,
    error: Option<String>,
}

struct RoutedImage {
    owner: WeakTexture,
    bridge: GpuBridge,
    intermediate: VulkanTextureResource,
    intermediate_valid: bool,
    pending: Option<(Texture, PresentableDraw, Stage)>,
}

#[derive(Clone, Copy)]
enum Stage {
    Sent,
    Received,
    Publishing,
}

impl CxVulkan {
    pub(super) fn is_routed_shared_image(&self, texture: TextureId) -> bool {
        self.desktop
            .hosted
            .as_ref()
            .is_some_and(|route| route.images.contains_key(&Self::texture_key(texture)))
    }

    pub(super) fn import_routed_shared_image(
        &mut self,
        texture: &Texture,
        width: u32,
        height: u32,
        image: LinuxOwnedImage,
    ) -> Result<(), String> {
        let identity = image
            .vulkan
            .as_ref()
            .ok_or("missing Vulkan transport identity")?
            .info;
        let mut route = match self.desktop.hosted.take() {
            Some(route) => route,
            None => HostedRoute {
                host: Box::new(Self::new_offscreen_on(Some(identity.device_uuid))?),
                images: HashMap::new(),
                error: None,
            },
        };
        let result = (|| {
            let current = route
                .host
                .gpu_snapshot()
                .renderer
                .ok_or("missing host GPU identity")?;
            if current.uuid != identity.device_uuid || current.driver_uuid != identity.driver_uuid {
                return Err("mixed compositor GPU identities in one framebuffer generation".into());
            }
            let key = Self::texture_key(texture.texture_id());
            if route.images.contains_key(&key) {
                return Err("duplicate routed framebuffer texture".into());
            }
            let local = self.create_color_target_resource(
                width,
                height,
                vk::Format::B8G8R8A8_UNORM,
                false,
            )?;
            let intermediate = match route.host.create_color_target_resource(
                width,
                height,
                vk::Format::B8G8R8A8_UNORM,
                false,
            ) {
                Ok(resource) => resource,
                Err(error) => {
                    self.destroy_texture_resource(local);
                    return Err(error);
                }
            };
            if let Err(error) = route
                .host
                .import_shared_image(texture, width, height, image)
            {
                self.destroy_texture_resource(local);
                route.host.destroy_texture_resource(intermediate);
                return Err(error);
            }
            let bridge = match GpuBridge::new(self, &route.host, vk::Extent2D { width, height }) {
                Ok(bridge) => bridge,
                Err(error) => {
                    self.destroy_texture_resource(local);
                    route.host.destroy_texture_resource(intermediate);
                    route.host.retire_unused_shared_producer(key);
                    return Err(error);
                }
            };
            self.textures.insert(key, local);
            route.images.insert(
                key,
                RoutedImage {
                    owner: texture.downgrade(),
                    bridge,
                    intermediate,
                    intermediate_valid: false,
                    pending: None,
                },
            );
            crate::log!(
                "Vulkan hosted route: app {:?} -> compositor {:?}, {width}x{height}, GPU transfers",
                self.desktop.gpu.device.uuid,
                current.uuid
            );
            Ok(())
        })();
        self.desktop.hosted = Some(route);
        result
    }

    pub(super) fn routed_shared_write_available(&self, texture: TextureId) -> Result<bool, String> {
        let route = self
            .desktop
            .hosted
            .as_ref()
            .ok_or("missing hosted GPU route")?;
        if route.error.is_some() {
            return Ok(false);
        }
        let image = route
            .images
            .get(&Self::texture_key(texture))
            .ok_or("missing hosted GPU image")?;
        if image.pending.is_some() || !route.host.shared_write_available(texture)? {
            return Ok(false);
        }
        unsafe { image.bridge.writable(self, &route.host) }
    }

    /// Called immediately after this app's draw submission. The Texture pin
    /// keeps that local image alive until every bridge and host copy completes.
    pub(crate) fn queue_routed_present(
        &mut self,
        texture: &Texture,
        draw: PresentableDraw,
    ) -> Result<bool, String> {
        let Some(mut route) = self.desktop.hosted.take() else {
            return Ok(false);
        };
        let result: Result<bool, String> = (|| {
            let key = Self::texture_key(texture.texture_id());
            let Some(image) = route.images.get_mut(&key) else {
                return Ok(false);
            };
            if image.pending.is_some() {
                return Err("routed framebuffer still has an outstanding presentation".into());
            }
            // Pin before queueing; an export failure may still leave accepted
            // GPU work whose source image must survive error retirement.
            image.pending = Some((texture.clone(), draw, Stage::Sent));
            unsafe { image.bridge.send(self, self.textures[&key].image) }?;
            Ok(true)
        })();
        if let Err(error) = &result {
            route.error = Some(error.clone());
        }
        self.desktop.hosted = Some(route);
        result
    }

    pub(crate) fn poll_routed_presents(&mut self) -> Result<Vec<PresentableDraw>, String> {
        let Some(mut route) = self.desktop.hosted.take() else {
            return Ok(Vec::new());
        };
        if route.error.is_some() {
            self.desktop.hosted = Some(route);
            return Ok(Vec::new());
        }
        let result: Result<Vec<PresentableDraw>, String> = (|| {
            let mut completed = Vec::new();
            let mut retired = Vec::new();
            for (&key, image) in &mut route.images {
                if !unsafe { image.bridge.poll_initialization(self, &route.host) }? {
                    continue;
                }
                if let Some((texture, draw, stage)) = image.pending.as_mut() {
                    if matches!(*stage, Stage::Sent)
                        && unsafe {
                            image.bridge.receive(
                                self,
                                &route.host,
                                image.intermediate.image,
                                image.intermediate_valid,
                            )
                        }?
                    {
                        image.intermediate_valid = true;
                        *stage = Stage::Received;
                    }
                    if matches!(*stage, Stage::Received)
                        && unsafe { image.bridge.idle(self, &route.host) }?
                        && route.host.hosted_main_idle()?
                    {
                        route
                            .host
                            .copy_routed_frame(texture.texture_id(), &image.intermediate)?;
                        draw.sequence = route.host.shared_draw_sequence(texture.texture_id());
                        *stage = Stage::Publishing;
                    }
                    if matches!(*stage, Stage::Publishing)
                        && route.host.hosted_main_idle()?
                        && route
                            .host
                            .shared_sequence_completed(texture.texture_id(), draw.sequence)?
                    {
                        completed.push(*draw);
                        image.pending = None;
                    }
                }
                if image.pending.is_none()
                    && image.owner.upgrade().is_none()
                    && unsafe { image.bridge.idle(self, &route.host) }?
                {
                    retired.push(key);
                }
            }
            for key in retired {
                let image = route.images.remove(&key).unwrap();
                unsafe { image.bridge.destroy(self, &route.host) };
                route.host.destroy_texture_resource(image.intermediate);
                route.host.retire_unused_shared_producer(key);
            }
            Ok(completed)
        })();
        if let Err(error) = &result {
            route.error = Some(error.clone());
        }
        self.desktop.hosted = Some(route);
        result
    }

    pub(super) fn hosted_routes_idle(&self) -> Result<bool, String> {
        let Some(route) = &self.desktop.hosted else {
            return Ok(true);
        };
        if !route.host.hosted_main_idle()? {
            return Ok(false);
        }
        for image in route.images.values() {
            if (image.pending.is_some() && route.error.is_none())
                || !unsafe { image.bridge.idle(self, &route.host) }?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn hosted_routes_ready(&mut self) -> Result<bool, String> {
        self.poll_routed_presents()?;
        let Some(route) = &self.desktop.hosted else {
            return Ok(true);
        };
        if let Some(error) = &route.error {
            return Err(error.clone());
        }
        for image in route.images.values() {
            if !unsafe { image.bridge.writable(self, &route.host) }? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn hosted_main_idle(&self) -> Result<bool, String> {
        if self.in_flight_fence == vk::Fence::null() {
            return Err("hosted GPU has no submission fence".into());
        }
        unsafe { self.device.get_fence_status(self.in_flight_fence) }
            .map_err(|e| format!("poll hosted output fence: {e:?}"))
    }

    fn copy_routed_frame(
        &mut self,
        texture: TextureId,
        source: &VulkanTextureResource,
    ) -> Result<(), String> {
        let key = Self::texture_key(texture);
        let destination = self.textures[&key].image;
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset hosted output commands: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin hosted output commands: {e:?}"))?;
        }
        self.acquire_shared_write(texture);
        self.transition_image_layout(
            destination,
            vk::ImageAspectFlags::COLOR,
            1,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        self.transition_image_layout(
            source.image,
            vk::ImageAspectFlags::COLOR,
            1,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        let layer = vk::ImageSubresourceLayers::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .layer_count(1);
        unsafe {
            self.device.cmd_copy_image(
                self.command_buffer,
                source.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(layer)
                    .dst_subresource(layer)
                    .extent(vk::Extent3D {
                        width: source.width,
                        height: source.height,
                        depth: 1,
                    })],
            );
        }
        self.transition_image_layout(
            source.image,
            vk::ImageAspectFlags::COLOR,
            1,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        if !self.release_shared_write_from(texture, vk::ImageLayout::TRANSFER_DST_OPTIMAL) {
            return Err("hosted output lost its producer timeline".into());
        }
        self.textures.get_mut(&key).unwrap().layout = vk::ImageLayout::GENERAL;
        unsafe { self.device.end_command_buffer(self.command_buffer) }
            .map_err(|e| format!("end hosted output commands: {e:?}"))?;
        let commands = [self.command_buffer];
        self.submit_frame(&vk::SubmitInfo::default().command_buffers(&commands))
    }

    pub(super) fn destroy_hosted_route(&mut self) {
        let Some(route) = self.desktop.hosted.take() else {
            return;
        };
        self.device_wait_idle();
        route.host.device_wait_idle();
        for (_, image) in route.images {
            unsafe { image.bridge.destroy(self, &route.host) };
            route.host.destroy_texture_resource(image.intermediate);
        }
        // Both devices still exist above; only now can the auxiliary host
        // context release its imports and Vulkan device.
    }
}
