
use crate::apple_nw_sys::*;
use makepad_objc_sys::objc_block;
use makepad_objc_sys::runtime::{ObjcId, nil};
use std::ffi::CString;

pub struct HttpsConnection {
}

impl HttpsConnection {
    pub fn connect(host_name: &str, port: &str) -> Self {unsafe{
        let hostname = CString::new(host_name).unwrap();
        let port = CString::new(port).unwrap();
        let parameters = nw_parameters_create_secure_tcp(_nw_parameters_configure_protocol_default_configuration, _nw_parameters_configure_protocol_default_configuration);
        // alright lets see.
        let endpoint = nw_endpoint_create_host(hostname.as_ptr(), port.as_ptr());
        let connection = nw_connection_create(endpoint, parameters);
        let queue = dispatch_queue_create("https\0".as_ptr(), nil);
        
        let mut state_change_handler = objc_block!(move | state: nw_connection_state, _error: ObjcId | {
            println!("{:?}", state);
            match state {
                nw_connection_state::invalid => {
                }
                nw_connection_state::waiting => {
                }
                nw_connection_state::preparing => {
                }
                nw_connection_state::ready => {
                    
                }
                nw_connection_state::failed => {
                    
                }
                nw_connection_state::cancelled => {
                    
                }
            }
        });
        nw_connection_set_queue(connection, queue);
        nw_connection_set_state_changed_handler(
            connection,
            &mut state_change_handler as *mut _ as ObjcId
        );
        Self {}
        /*
        const char *hostname = "imap.gmail.com";
        const char *port = "imaps";
        nw_parameters_t parameters = nw_parameters_create_secure_tcp(NW_PARAMETERS_DEFAULT_CONFIGURATION, NW_PARAMETERS_DEFAULT_CONFIGURATION);
        nw_endpoint_t endpoint = nw_endpoint_create_host(hostname, port);
        nw_connection_t connection = nw_connection_create(endpoint, parameters);
        
        nw_connection_set_queue(connection, dispatch_get_main_queue());
        nw_connection_set_state_changed_handler(connection, ^(nw_connection_state_t state, nw_error_t error) {
            switch (state) {
                case nw_connection_state_waiting:
                    NSLog(@"waiting");
                    break;
                case nw_connection_state_failed:
                    NSLog(@"failed");
                    break;
                case nw_connection_state_ready:
                    NSLog(@"connection is ready");
                    break;
                case nw_connection_state_cancelled:
                    NSLog(@"connection is cancelled");
                    break;
                default:
                    break;
            }
        });
        nw_connection_start(connection);*/
    }}
}
