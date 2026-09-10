// All image IO/decoding stays in the existing iteration worker. UI snapshots
// carry small Arc previews; original images remain private evidence on disk.
#[derive(Clone)]
pub struct Attachment {
    pub id: String,
    pub flow: String,
    pub path: PathBuf,
    pub delivered: bool,
    pub submitted: bool,
}
impl Attachment {
    fn json(&self) -> Value {
        json::obj(vec![
            ("id", s(&self.id)),
            ("flow", s(&self.flow)),
            ("path", s(self.path.to_string_lossy())),
            ("delivered", Value::Bool(self.delivered)),
            ("submitted", Value::Bool(self.submitted)),
        ])
    }
}
pub struct Preview {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u32>>,
}

impl Host {
    /// Resolve only evidence already registered to this lane. Callers never
    /// provide an arbitrary filesystem path to the media viewer.
    fn evidence_preview_path(&self, flow: &str, id: &str) -> Result<PathBuf, String> {
        let flow_state = self.flow(flow)?;
        let path = flow_state.captures.iter().find(|capture| capture.id == id)
            .map(|capture| capture.path.clone())
            .or_else(|| self.attachments.iter()
                .find(|attachment| self.attachment_owner(attachment) == flow && attachment.id == id)
                .map(|attachment| attachment.path.clone()))
            .ok_or("Image does not belong to this lane")?;
        recording_owned_path(&self.directory, &path)?;
        Ok(path)
    }

    fn load_full_evidence_preview(&self, flow: &str, id: &str) -> Result<Arc<Preview>, String> {
        let path = self.evidence_preview_path(flow, id)?;
        let bytes = recording_bytes(&self.directory, &path, 64 * 1024 * 1024)?;
        let image = decode_evidence_image(&bytes, &path)?;
        if let Some(capture) = self.flow(flow)?.captures.iter().find(|capture| capture.id == id) {
            if image.width != capture.width as usize || image.height != capture.height as usize {
                return Err("Image dimensions no longer match the registered capture".into());
            }
        }
        Ok(Arc::new(Preview {
            id: id.to_owned(),
            width: image.width as u32,
            height: image.height as u32,
            pixels: Arc::new(image.data),
        }))
    }

    fn presentation_json(&self) -> Value {
        Value::Obj(
            self.terminal_heights
                .iter()
                .map(|(id, height)| (id.clone(), Value::F64(*height)))
                .collect(),
        )
    }
    fn lane_widths_json(&self) -> Value {
        Value::Obj(self.lane_widths.iter().map(|(flow, width)| (flow.clone(), Value::F64(*width))).collect())
    }
    fn restore_media(&mut self, value: &Value) -> Result<(), String> {
        if let Some(Value::Obj(items)) = value.get("lane_widths") {
            for (flow, value) in items.iter().take(iteration::MAX_STORED_FLOWS) {
                let width = match value {
                    Value::F64(width) => *width,
                    Value::Int(width) => *width as f64,
                    _ => continue,
                };
                if cli_identifier(flow) && width.is_finite() {
                    self.lane_widths.insert(flow.clone(), width.clamp(420.0, 1600.0));
                }
            }
        }
        if let Some(Value::Obj(items)) = value.get("design_snapshots") {
            for (run, v) in items.iter().take(128) {
                if let Some(items) = v.as_arr() {
                    if let (Some(generation), Some(id)) = (
                        items.first().and_then(Value::as_u64),
                        items.get(1).and_then(Value::as_str),
                    ) {
                        self.design_snapshots
                            .insert(run.clone(), (generation, id.into()));
                    }
                }
            }
        }
        if let Some(Value::Arr(items)) = value.get("attachments") {
            if items.len() > 512 {
                return Err("Attachment history exceeds its bound".into());
            }
            for item in items {
                let field = |key| {
                    item.get(key)
                        .and_then(Value::as_str)
                        .ok_or("Malformed attachment history")
                };
                let path = PathBuf::from(field("path")?);
                if !path.is_absolute()
                    || !path.starts_with(&self.directory)
                    || path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err("Attachment escaped private history".into());
                }
                self.attachments.push(Attachment {
                    id: field("id")?.into(),
                    flow: field("flow")?.into(),
                    path,
                    delivered: item
                        .get("delivered")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    submitted: item.get("submitted").and_then(Value::as_bool).unwrap_or(false),
                });
            }
        }
        if let Some(Value::Obj(items)) = value.get("presentation") {
            for (id, v) in items.iter().take(4) {
                let height = match v {
                    Value::F64(v) => *v,
                    Value::Int(v) => *v as f64,
                    _ => continue,
                };
                if height.is_finite() {
                    self.terminal_heights
                        .insert(id.clone(), height.clamp(64.0, 640.0));
                }
            }
        }
        if let Some(Value::Obj(reports)) = value.get("reports") {
            self.reports.extend(reports.iter().take(128).cloned());
        }
        Ok(())
    }
    fn import_attachment(
        &mut self,
        flow: &str,
        path: &Path,
        delivered: bool,
    ) -> Result<Value, String> {
        self.flow(flow)?;
        if self.attachments.len() >= 512 {
            return Err("Attachment history is full".into());
        }
        let image = read_evidence_image(path)?;
        let mut rgba = Vec::with_capacity(image.width * image.height * 4);
        for pixel in &image.data[..image.width * image.height] {
            rgba.extend_from_slice(&[
                ((pixel >> 16) & 255) as u8,
                ((pixel >> 8) & 255) as u8,
                (pixel & 255) as u8,
                (pixel >> 24) as u8,
            ]);
        }
        let png = makepad_widgets::Cx::encode_rgba_as_png(
            image.width as u32,
            image.height as u32,
            &rgba,
        )?;
        let id = format!("attachment-{}-{}", now(), self.attachments.len());
        let destination = self.directory.join("attachments").join(format!("{id}.png"));
        atomic_write(&destination, &png)?;
        self.attachments.push(Attachment {
            id: id.clone(),
            flow: flow.into(),
            path: destination.clone(),
            delivered,
            submitted: false,
        });
        if let Err(error) = self.persist() {
            self.attachments.pop();
            let _ = fs::remove_file(destination);
            return Err(error);
        }
        self.changed = true;
        self.note = if delivered {
            "Image sent to terminal; evidence saved"
        } else {
            "Image saved; waiting for the backing terminal"
        }
        .into();
        Ok(json::obj(vec![
            ("id", s(id)),
            ("delivered", Value::Bool(delivered)),
        ]))
    }
    fn attachment_delivered(&mut self, id: &str) -> Result<Value, String> {
        let index = self
            .attachments
            .iter()
            .position(|a| a.id == id)
            .ok_or("Unknown attachment")?;
        let previous = self.attachments[index].delivered;
        self.attachments[index].delivered = true;
        if let Err(error) = self.persist() {
            self.attachments[index].delivered = previous;
            return Err(error);
        }
        self.changed = true;
        Ok(Value::Bool(true))
    }
    fn clear_attachment_tray(&mut self, flow: &str) -> Result<Value, String> {
        if !self.engine.flows.contains_key(flow) { return Err("Unknown flow".into()); }
        let changed: Vec<_> = self.attachments.iter().enumerate().filter(|(_,a)| self.attachment_owner(a) == flow && a.delivered && !a.submitted).map(|(i,_)|i).collect();
        for index in &changed { self.attachments[*index].submitted = true; }
        if let Err(error) = self.persist() {
            for index in &changed { self.attachments[*index].submitted = false; }
            return Err(error);
        }
        self.changed = true;
        Ok(json::obj(vec![("flow", s(flow)), ("hidden_thumbnails", Value::Int(changed.len() as i64))]))
    }
    fn load_previews(&mut self) {
        let paths: Vec<_> = self
            .attachments
            .iter()
            .map(|a| (a.id.clone(), a.path.clone()))
            .chain(
                self.engine
                    .flows
                    .values()
                    .flat_map(|f| f.captures.iter().map(|c| (c.id.clone(), c.path.clone()))),
            )
            .collect();
        for (id, path) in paths {
            if self.previews.contains_key(&id) || self.preview_failed.contains(&id) {
                continue;
            }
            // 128 × 256² pixels is at most 32 MiB, shared with UI snapshots.
            if self.previews.len() >= 128 {
                break;
            }
            match read_evidence_image(&path) {
                Ok(image) => {
                    let scale = (256.0 / image.width.max(image.height) as f64).min(1.0);
                    let width = (image.width as f64 * scale).round().max(1.0) as usize;
                    let height = (image.height as f64 * scale).round().max(1.0) as usize;
                    let mut pixels = Vec::with_capacity(width * height);
                    for y in 0..height {
                        for x in 0..width {
                            pixels.push(
                                image.data[(y * image.height / height) * image.width
                                    + x * image.width / width],
                            );
                        }
                    }
                    self.previews.insert(
                        id.clone(),
                        Arc::new(Preview {
                            id,
                            width: width as u32,
                            height: height as u32,
                            pixels: Arc::new(pixels),
                        }),
                    );
                    self.changed = true;
                }
                Err(error) => {
                    self.preview_failed.insert(id);
                    self.note = format!("Image preview unavailable: {error}");
                    self.changed = true;
                }
            }
        }
    }
}
fn read_evidence_image(path: &Path) -> Result<makepad_widgets::image_cache::ImageBuffer, String> {
    if !path.is_absolute() {
        return Err("Image path must be absolute".into());
    }
    let meta = fs::symlink_metadata(path).map_err(err)?;
    if !meta.file_type().is_file() || meta.len() > 64 * 1024 * 1024 {
        return Err("Image must be a regular file under 64 MiB".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(err)?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Image grew beyond 64 MiB".into());
    }
    decode_evidence_image(&bytes, path)
}

fn decode_evidence_image(bytes: &[u8], path: &Path) -> Result<makepad_widgets::image_cache::ImageBuffer, String> {
    if !(bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")))
    {
        return Err("Only PNG, JPEG and WebP image attachments are supported".into());
    }
    let (width, height) =
        makepad_widgets::image_cache::image_size_by_data(bytes, path).map_err(err)?;
    if width == 0 || height == 0 || width.saturating_mul(height) > 32 * 1024 * 1024 {
        return Err("Image exceeds 32 megapixels".into());
    }
    let image = makepad_widgets::image_cache::decode_image_from_data(bytes).map_err(err)?;
    if image.width != width || image.height != height || image.data.len() != width * height {
        return Err("Decoded image dimensions are inconsistent".into());
    }
    Ok(image)
}
