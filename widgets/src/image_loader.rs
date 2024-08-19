use std::fmt::{self, Display, Formatter};

use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, ContentLoader, ContentLoadingStatus,
    ImageWidgetExt, LabelWidgetExt, WidgetMatchEvent,
};

live_design! {
    ImageLoaderBase = {{ImageLoader}} {}
}

#[derive(Clone, Debug, DefaultNone)]
pub enum ImageLoaderAction {
    None,
    ImageLoaded,
    ImageLoadingFailed,
}

#[derive(Live, LiveHook, Widget)]
pub struct ImageLoader {
    #[deref]
    content_loader: ContentLoader,

    #[rust]
    uri: Option<String>,
    #[rust]
    image_loading_status: ImageLoadingStatus,
}

#[derive(Default)]
pub enum ImageLoadingStatus {
    #[default]
    NotStarted,
    Loading,
    Loaded,
    Failed,
}

impl Widget for ImageLoader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.content_loader.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.content_loader.draw_walk(cx, scope, walk)
    }
}

impl WidgetMatchEvent for ImageLoader {
    fn handle_network_responses(
        &mut self,
        cx: &mut Cx,
        responses: &NetworkResponsesEvent,
        scope: &mut Scope,
    ) {
        if self.uri.is_none() {
            return;
        }
        for event in responses {
            if event.request_id == live_id!(image_loader_request) {
                match &event.response {
                    NetworkResponse::HttpResponse(response) => {
                        let uri = self.uri.as_ref().unwrap();
                        let is_expected_response = response.metadata_id == LiveId::from_str(&uri);
                        if response.status_code == 200 && is_expected_response {
                            if let Some(body) = response.get_body() {
                                // TODO: Caching
                                // cx.get_global::<NetworkImageCache>()
                                //     .insert(response.metadata_id, body);

                                // TODO: instead of content being pub, use a callback with imageref access like in text_or_image
                                let image_ref = self.content_loader.content.image(id!(image));

                                let image_format = get_image_format(&body);
                                match image_format {
                                    Some(ImageFormat::PNG) => {
                                        let _ = image_ref.load_png_from_data(cx, &body);
                                        self.content_loader.set_loaded_and_show_content(cx);
                                        self.image_loading_status = ImageLoadingStatus::Loaded;
                                    }
                                    Some(ImageFormat::JPEG) => {
                                        let _ = image_ref.load_jpg_from_data(cx, &body);
                                        self.content_loader.set_loaded_and_show_content(cx);
                                        self.image_loading_status = ImageLoadingStatus::Loaded;
                                    }
                                    None => {
                                        let error = "Failed to determine image format";
                                        error!("{error}");
                                        self.fail_image_loading(cx, &error);
                                    }
                                    _ => {
                                        let error = format!(
                                            "Unsupported image format: {}",
                                            image_format.unwrap()
                                        );
                                        error!("{error}");
                                        self.fail_image_loading(cx, &error);
                                    }
                                }

                                cx.widget_action(
                                    self.widget_uid(),
                                    &scope.path,
                                    ImageLoaderAction::ImageLoaded,
                                );
                                self.redraw(cx);
                            }
                        } else {
                            self.fail_image_loading(cx, "Failed to fetch image");
                        }
                    }
                    NetworkResponse::HttpRequestError(error) => {
                        error!("Error fetching gallery image: {:?}", error);
                    }
                    _ => (),
                }
            }
        }
    }
}

impl ImageLoader {
    fn fail_image_loading(&mut self, cx: &mut Cx, error: &str) {
        self.image_loading_status = ImageLoadingStatus::Failed;
        self.content_loader
            .set_content_loading_status(ContentLoadingStatus::Failed);
        self.content_loader
            .error
            .label(id!(error_message))
            .set_text(&error);
        cx.widget_action(
            self.widget_uid(),
            &HeapLiveIdPath::default(),
            ImageLoaderAction::ImageLoadingFailed,
        );
        self.redraw(cx);
    }

    pub fn load_from_url(&mut self, cx: &mut Cx, uri: &str) {
        self.image_loading_status = ImageLoadingStatus::Loading;
        self.content_loader
            .set_content_loading_status(ContentLoadingStatus::Loading);
        self.content.redraw(cx);

        let mut request = HttpRequest::new(uri.to_string(), HttpMethod::GET);
        request.metadata_id = LiveId::from_str(&uri);
        cx.http_request(live_id!(image_loader_request), request);
        self.uri = Some(uri.to_string());
    }
}

impl ImageLoaderRef {
    pub fn load_from_url(&mut self, cx: &mut Cx, uri: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_from_url(cx, uri);
        }
    }
}

// TODO: Find a better place for these
#[derive(Debug, PartialEq)]
pub enum ImageFormat {
    PNG,
    JPEG,
    GIF,
    BMP,
    ICO,
    WebP,
    TIFF,
}

impl Display for ImageFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ImageFormat::PNG => write!(f, "PNG"),
            ImageFormat::JPEG => write!(f, "JPEG"),
            ImageFormat::GIF => write!(f, "GIF"),
            ImageFormat::BMP => write!(f, "BMP"),
            ImageFormat::ICO => write!(f, "ICO"),
            ImageFormat::WebP => write!(f, "WebP"),
            ImageFormat::TIFF => write!(f, "TIFF"),
        }
    }
}

fn get_image_format(data: &[u8]) -> Option<ImageFormat> {
    if data.len() < 8 {
        // Not enough data to determine format
        return None;
    }

    match &data[0..8] {
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] => Some(ImageFormat::PNG),
        [0xFF, 0xD8, 0xFF, ..] => Some(ImageFormat::JPEG),
        [0x47, 0x49, 0x46, 0x38, 0x37, 0x61, ..] | [0x47, 0x49, 0x46, 0x38, 0x39, 0x61, ..] => {
            Some(ImageFormat::GIF)
        }
        [0x42, 0x4D, ..] => Some(ImageFormat::BMP),
        [0x00, 0x00, 0x01, 0x00, ..] => Some(ImageFormat::ICO),
        [0x52, 0x49, 0x46, 0x46, _, _, _, _, 0x57, 0x45, 0x42, 0x50] if data.len() >= 12 => {
            Some(ImageFormat::WebP)
        }
        [0x49, 0x49, 0x2A, 0x00] | [0x4D, 0x4D, 0x00, 0x2A] => Some(ImageFormat::TIFF),
        _ => None,
    }
}
