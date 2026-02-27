use crate::types::{HttpRequest, HttpResponse, NetworkResponse};
use makepad_futures_legacy::executor;
use makepad_live_id::LiveId;
use std::sync::mpsc::Sender;

use windows::{
    core::Interface,
    Foundation::Uri,
    Storage::Streams::{Buffer, DataWriter, InMemoryRandomAccessStream, InputStreamOptions},
    Web::Http::{
        HttpClient, HttpCompletionOption, HttpMethod, HttpRequestMessage, HttpStreamContent,
        IHttpContent,
    },
    Win32::System::WinRT::IBufferByteAccess,
};

pub struct WindowsHttpSocket;

impl WindowsHttpSocket {
    pub fn open(request_id: LiveId, request: HttpRequest, response_sender: Sender<NetworkResponse>) {
        async fn create_request(
            request: &HttpRequest,
        ) -> windows::core::Result<HttpRequestMessage> {
            let uri = Uri::CreateUri(&request.url.to_string().into())?;
            let req = HttpRequestMessage::Create(
                &HttpMethod::Create(&request.method.as_str().into())?,
                &uri,
            )?;

            let headers_map = req.Headers()?;
            let mut content_type = None;
            for (key, values) in request.headers.iter() {
                for value in values {
                    match key.as_str() {
                        "Content-Type" => {
                            content_type = Some(value.clone());
                        }
                        _ => {
                            headers_map.Append(&key.into(), &value.into())?;
                        }
                    }
                }
            }

            if let Some(body) = &request.body {
                let stream = InMemoryRandomAccessStream::new()?;
                let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0)?)?;
                writer.WriteBytes(&body)?;
                writer.StoreAsync()?.await?;
                writer.FlushAsync()?.await?;
                stream.Seek(0)?;

                let content = HttpStreamContent::CreateFromInputStream(&stream)?;
                let headers_map = content.Headers()?;
                if let Some(content_type) = content_type {
                    headers_map.Append(&"Content-Type".into(), &content_type.into())?;
                }

                req.SetContent(&content.cast::<IHttpContent>()?)?;
            }

            Ok(req)
        }

        async fn streaming_request(
            request_id: LiveId,
            request: HttpRequest,
            response_sender: Sender<NetworkResponse>,
        ) -> windows::core::Result<()> {
            let client = HttpClient::new()?;
            let req = create_request(&request).await?;
            let response = client
                .SendRequestWithOptionAsync(&req, HttpCompletionOption::ResponseHeadersRead)?
                .await?;

            let input_stream = response.Content()?.ReadAsInputStreamAsync()?.await?;
            let buffer = Buffer::Create(1024 * 1024)?;
            loop {
                input_stream
                    .ReadAsync(&buffer, buffer.Capacity()?, InputStreamOptions::Partial)?
                    .await?;
                let chunk_size = buffer.Length()?;
                if chunk_size == 0 {
                    break;
                }
                let byte_access: IBufferByteAccess = buffer.cast()?;
                let chunk = unsafe {
                    std::slice::from_raw_parts(
                        byte_access.Buffer()? as *const u8,
                        chunk_size as usize,
                    )
                };
                let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                    request_id,
                    response: HttpResponse {
                        headers: Default::default(),
                        metadata_id: request.metadata_id,
                        status_code: 0,
                        body: Some(chunk.to_vec()),
                    },
                });
            }
            let _ = response_sender.send(NetworkResponse::HttpStreamComplete {
                request_id,
                response: HttpResponse {
                    headers: Default::default(),
                    metadata_id: request.metadata_id,
                    status_code: 0,
                    body: None,
                },
            });
            Ok(())
        }

        async fn non_streaming_request(
            request_id: LiveId,
            request: HttpRequest,
            response_sender: Sender<NetworkResponse>,
        ) -> windows::core::Result<()> {
            let client = HttpClient::new()?;
            let req = create_request(&request).await?;
            let response = client
                .SendRequestWithOptionAsync(&req, HttpCompletionOption::ResponseHeadersRead)?
                .await?;

            let buffer = response.Content()?.ReadAsBufferAsync()?.await?;
            let byte_access: IBufferByteAccess = buffer.cast()?;
            let chunk_size = buffer.Length()?;
            let chunk = unsafe {
                std::slice::from_raw_parts(byte_access.Buffer()? as *const u8, chunk_size as usize)
            };
            let _ = response_sender.send(NetworkResponse::HttpResponse {
                request_id,
                response: HttpResponse {
                    headers: Default::default(),
                    metadata_id: request.metadata_id,
                    status_code: 0,
                    body: Some(chunk.to_vec()),
                },
            });
            Ok(())
        }

        let _reader_thread = std::thread::spawn(move || {
            if request.is_streaming {
                let _ = executor::block_on(streaming_request(request_id, request, response_sender));
            } else {
                let _ = executor::block_on(non_streaming_request(request_id, request, response_sender));
            }
        });
    }
}
