use{
    crate::{
        os::{
            linux::{
                openxr_sys::*,
                openxr::*,
            }
        },
        cx::OsType,
        makepad_micro_serde::*,
        event::xr::*,
        makepad_math::{Pose, Quat},
    },
    std::path::Path,
};

impl CxOpenXr{
        
    pub fn advertise_anchor(&mut self, _anchor:XrAnchor){
        if let Some(_session) = &mut self.session{
            let _xr =  self.libxr.as_ref().unwrap();
            crate::log!("Creating advertising anchor");
                        
            //ession.create_shareable_anchor_request(anchors, xr);
        }
    }
        
    pub fn set_local_anchor(&mut self, anchor:XrAnchor){
        if let Some(session) = &mut self.session{
            let xr =  self.libxr.as_ref().unwrap();
            crate::log!("Setting local anchor");
            session.set_local_anchor(xr, anchor);
            //session.local_anchor = Some(pose);
            //session.create_local_anchor_request(pose, xr);
            //session.create_shareable_anchor_request(pose, xr);
        }
    }
            
    pub fn get_local_anchor(&mut self, os_type:&OsType){
        if let Some(session) = &mut self.session{
            let xr =  self.libxr.as_ref().unwrap();
            crate::log!("Getting local anchor");
            session.get_local_anchor(xr, os_type);
        }
    }
    pub fn discover_anchor(&mut self, _id:u8){
        if let Some(_session) = &mut self.session{
            let _xr =  self.libxr.as_ref().unwrap();
            //session.anchor_discovery = id;
            //crate::log!("Discovering anchor {id}");
            //session.start_colocation_discovery_request(xr);
        }
    }
}

impl CxOpenXrSession{
    
    pub(crate) fn handle_anchor_events(&mut self, xr:&LibOpenXr, event_buffer: &XrEventDataBuffer, os_type:&OsType){
         match event_buffer.ty{
            XrStructureType::EVENT_DATA_SPATIAL_ANCHOR_CREATE_COMPLETE_FB=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataSpatialAnchorCreateCompleteFB)};
                self.create_anchor_response(xr, response, os_type).ok();
            }
            XrStructureType::EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataSpaceSetStatusCompleteFB)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB: {:?}", response.result);
                }
            }
            XrStructureType::EVENT_DATA_SHARE_SPACES_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataShareSpacesCompleteMETA)};
                self.share_spaces_response(xr, response).ok();
            }
            XrStructureType::EVENT_DATA_START_COLOCATION_DISCOVERY_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataStartColocationDiscoveryCompleteMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("START_COLOCATION_DISCOVERY_COMPLETE_META: {:?}", response.result);
                }
            },
            XrStructureType::EVENT_DATA_COLOCATION_DISCOVERY_RESULT_META=>{
                let _response = &unsafe{*(event_buffer as *const _ as *const XrEventDataColocationDiscoveryResultMETA)};
                /*openxr.session.as_mut().unwrap().start_colocation_discovery_result(
                    openxr.libxr.as_ref().unwrap(),
                    response,
                );*/
            },
            XrStructureType::EVENT_DATA_COLOCATION_DISCOVERY_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataColocationDiscoveryCompleteMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("COLOCATION_DISCOVERY_COMPLETE_META: {:?}", response.result);
                }
            },
            XrStructureType::EVENT_DATA_STOP_COLOCATION_DISCOVERY_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataStopColocationDiscoveryCompleteMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("STOP_COLOCATION_DISCOVERY_COMPLETE_META: {:?}", response.result);
                }
            },
            XrStructureType::EVENT_DATA_START_COLOCATION_ADVERTISEMENT_COMPLETE_META=>{
                let _response = &unsafe{*(event_buffer as *const _ as *const XrEventDataStartColocationAdvertisementCompleteMETA)};
                /*if response.result != XrResult::SUCCESS{
                    crate::log!("START_COLOCATION_ADVERTISEMENT_COMPLETE_META: {:?} {:?}", response.result, response.advertisement_uuid);
                }
                else{
                    // this signals others to reload the colocation
                    openxr.session.as_mut().unwrap().anchor_discovery += 1;
                }*/
            }
            XrStructureType::EVENT_DATA_COLOCATION_ADVERTISEMENT_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataColocationAdvertisementCompleteMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("COLOCATION_ADVERTISEMENT_COMPLETE_META: {:?}", response.result);
                }
                    
            }
            XrStructureType::EVENT_DATA_STOP_COLOCATION_ADVERTISEMENT_COMPLETE_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataStopColocationAdvertisementCompleteMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("STOP_COLOCATION_ADVERTISEMENT_COMPLETE_META: {:?}", response.result);
                }
            }
            XrStructureType::EVENT_DATA_SPACE_QUERY_RESULTS_AVAILABLE_FB=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataSpaceQueryResultsAvailableFB)};
                if let Err(e) = self.query_spaces_response(xr,response){
                    crate::log!("query_anchors_response: {:?}", e);
                }
            }
            XrStructureType::EVENT_DATA_SPACES_SAVE_RESULT_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataSpacesSaveResultMETA)};                
                if let Err(e) = self.save_spaces_response(xr,response){
                    crate::log!("save_spaces_response: {:?}", e);
                }
                
            }
            XrStructureType::EVENT_DATA_SPACES_ERASE_RESULT_META=>{
                let response = &unsafe{*(event_buffer as *const _ as *const XrEventDataSpacesEraseResultMETA)};
                if response.result != XrResult::SUCCESS{
                    crate::log!("EVENT_DATA_SPACES_ERASE_RESULT_META: {:?}", response.result);
                }
            }
            _=>()
        }
    }   
    
    pub fn advertise_anchors(&mut self, _xr: &LibOpenXr, _anchor:XrAnchor){
        //self.create_shareable_anchor_request(anchors, xr);
    }
    
    pub fn set_local_anchor(&mut self, xr: &LibOpenXr, anchor:XrAnchor){
        // alright lets set the local anchor
        let left_pose = Pose{orientation: Quat::default(), position: anchor.left};
        let right_pose = Pose{orientation: Quat::default(), position: anchor.right};
        if !self.anchor.async_state.idle(){
            return crate::log!("set_local_anchor - async state not idle {:?}", self.anchor.async_state);
        }
        // delete old anchors
        if let Some(anchor) = self.anchor.anchor.take(){
            self.erase_spaces_request(xr, &[anchor.left_space, anchor.right_space]).ok();
        }
            
        if let Ok(request_id) = self.create_anchor_request(xr, left_pose){
            self.anchor.async_state = AsyncAnchorState::LocalAnchorLeft{
                request_id, 
                right_pose: right_pose
            }
        }
    }
    
    
    pub fn get_local_anchor(&mut self, xr: &LibOpenXr, os_type:&OsType){
        //let group_uuid = XrUuid::from_live_id(live_id!(makepad_space));
        
        let path = Path::new(&os_type.get_cache_dir().unwrap()).join("left_anchor");
        if let Ok(left_data) = std::fs::read(path){
            let path = Path::new(&os_type.get_cache_dir().unwrap()).join("right_anchor");
            if let Ok(right_data) = std::fs::read(path){
                let left_uuid = XrUuid::from_bytes(&left_data);
                let right_uuid = XrUuid::from_bytes(&right_data);
                if let Ok(request_id) = self.query_local_spaces_request(xr, [left_uuid, right_uuid]){
                    self.anchor.async_state = AsyncAnchorState::GetLocalAnchor{
                        left_uuid,
                        right_uuid,
                        request_id, 
                    }
                }
            }
        }
        
        
    }
    
    pub fn discover_anchor(&mut self, _id:u8){
        //if let Some(session) = &mut self.session{
        //    let xr =  self.libxr.as_ref().unwrap();
        //    session.anchor_discovery = id;
       //     crate::log!("Discovering anchor {id}");
       //     session.start_colocation_discovery_request(xr);
        //}
    }
    
    fn create_anchor_request(&self, xr: &LibOpenXr, pose:Pose)->Result<XrAsyncRequestIdFB,String>{
        let anchor_create_info = XrSpatialAnchorCreateInfoFB{
            space: self.local_space,
            pose_in_space: pose,
            time: self.frame_state.predicted_display_time,
            ..Default::default()
        };
        let mut request_id = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrCreateSpatialAnchorFB)(
            self.handle,
            &anchor_create_info,
            &mut request_id
        )}.to_result("xrCreateSpatialAnchorFB")?;
        return Ok(request_id)
    }
    
    fn create_anchor_response(&mut self, xr: &LibOpenXr, response: &XrEventDataSpatialAnchorCreateCompleteFB, os_type:&OsType)->Result<(),String>{
        match self.anchor.async_state{
            AsyncAnchorState::LocalAnchorLeft{right_pose,request_id}=>{
                self.anchor.async_state.result(response.result)?;
                self.anchor.async_state.error(response.request_id != request_id,"request_id")?;
                // lets write the left anchor uuid to disk
                let path = Path::new(&os_type.get_cache_dir().unwrap()).join("left_anchor");
                std::fs::write(path, response.uuid.data).ok();
                
                if let Ok(request_id) = self.create_anchor_request(xr, right_pose){
                    self.anchor.async_state = AsyncAnchorState::LocalAnchorRight{
                        request_id, 
                        left_space: response.space
                    };
                }
                else{
                    self.anchor.async_state.error(true,"create_anchor_request")?;
                }
            },
            AsyncAnchorState::LocalAnchorRight{left_space, request_id}=>{
                let right_space = response.space;
                self.anchor.async_state.result(response.result)?;
                self.anchor.async_state.error(response.request_id != request_id,"request_id")?;
                
                let path = Path::new(&os_type.get_cache_dir().unwrap()).join("right_anchor");
                std::fs::write(path, response.uuid.data).ok();
                if let Err(e) = self.set_space_status_request(xr, &[left_space, right_space], &[
                    XrSpaceComponentTypeFB::STORABLE,
                   // XrSpaceComponentTypeFB::SHARABLE,
                ]){
                    self.anchor.async_state.error(true,&format!("set_space_status_request - {:?}",e))?;
                }
                else{
                    //let group_uuid = XrUuid::from_live_id(live_id!(makepad_space));
                    self.anchor.anchor = Some(SpaceAnchor{
                        left_space,
                        right_space
                    });
                                        
                    match self.save_spaces_request(xr, [left_space, right_space]){
                        Ok(request_id)=>{
                            self.anchor.async_state = AsyncAnchorState::LocalAnchorSaveSpaces{
                                request_id
                            };
                        },
                        Err(e)=>{
                            self.anchor.async_state.error(true,&format!("set_space_status_response {e}")).ok();
                        }
                    }
                }
            },
            _=>{
                self.anchor.async_state.error(true,"create_anchor_response - unexpected")?;
            }
        }
        Ok(())
    }
        
    fn set_space_status_request(&mut self, xr: &LibOpenXr, spaces: &[XrSpace], flags: &[XrSpaceComponentTypeFB])->Result<(),String>{
        for space in spaces{
            let components = xr_array_fetch(XrSpaceComponentTypeFB::default(), |cap, len, buf|{
                unsafe{(xr.xrEnumerateSpaceSupportedComponentsFB)(
                    *space,
                    cap,
                    len, 
                    buf
                )}.to_result("xrEnumerateSpaceSupportedComponentsFB")
            }).unwrap_or(vec![]);
            
            for flag in flags{
                if !components.iter().any(|v| *v == *flag) {
                    crate::log!("share_anchor_response space not {:?}", flag);
                    return Err("set_space_flags_request".into())
                }
            }
            for flag in flags{            
                let request = XrSpaceComponentStatusSetInfoFB{
                    component_type: *flag,
                    enabled: XrBool32::from_bool(true),
                    ..Default::default()
                };
                let mut request_id = XrAsyncRequestIdFB(0);
                unsafe{(xr.xrSetSpaceComponentStatusFB)(
                    *space,
                    &request,
                    &mut request_id
                )}.log_error("xrSetSpaceComponentStatusFB");
            }
        }
        return Ok(())
    }
    
        
    fn save_spaces_request(&self, xr: &LibOpenXr, spaces:[XrSpace;2])->Result<XrAsyncRequestIdFB,String>{
        let spaces_info = XrSpacesSaveInfoMETA{
            space_count: 2,
            spaces: spaces.as_ptr(),
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrSaveSpacesMETA)(
            self.handle,
            &spaces_info,
            &mut request
        )}.to_result("xrSaveSpacesMETA")?;
        Ok(request)
    }
    
    fn save_spaces_response(&mut self, _xr: &LibOpenXr, response:&XrEventDataSpacesSaveResultMETA)->Result<(),String>{
        match self.anchor.async_state{
            AsyncAnchorState::LocalAnchorSaveSpaces{request_id}=>{
                self.anchor.async_state.result(response.result)?;
                self.anchor.async_state.error(response.request_id != request_id,"request_id")?;
                self.anchor.async_state = AsyncAnchorState::Idle;
                crate::log!("Save local spaces completed");
            }
            _=>{
                self.anchor.async_state.error(true,"share_spaces_response - unexpected")?;
            }
        }
        Ok(())
    }
    
    fn _share_spaces_request(&self, xr: &LibOpenXr, group_uuid: XrUuid, spaces:[XrSpace;2])->Result<XrAsyncRequestIdFB,String>{

        let recipient_info = XrShareSpacesRecipientGroupsMETA{
            group_count: 1,
            groups: &group_uuid,
            ..Default::default()
        };
        let spaces_info = XrShareSpacesInfoMETA{
            space_count: 2,
            spaces: spaces.as_ptr(),
            recipient_info: &recipient_info as *const _ as *const _,
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrShareSpacesMETA)(
            self.handle,
            &spaces_info,
            &mut request
        )}.to_result("xrShareSpacesMETA")?;
        Ok(request)
    }
        
    fn share_spaces_response(&mut self, _xr: &LibOpenXr, response:&XrEventDataShareSpacesCompleteMETA)->Result<(),String>{
        match self.anchor.async_state{
            AsyncAnchorState::LocalAnchorShareSpaces{request_id}=>{
                self.anchor.async_state.result(response.result)?;
                self.anchor.async_state.error(response.request_id != request_id,"request_id")?;
                crate::log!("Share local spaces completed");
            }
            _=>{
                self.anchor.async_state.error(true,"share_spaces_response - unexpected")?;
            }
        }
        Ok(())
    }
        
    fn _start_colocation_advertisement_request(&mut self, xr: &LibOpenXr, advertisement:AnchorAdvertisement){
        //self.stop_colocation_advertisement_request(xr);
        //self.stop_colocation_discovery_request(xr);
        //let anchor_advertisement = self.anchor_advertisement.as_ref().unwrap();
        let buffer = advertisement.serialize_bin();
        let advertisement_info = XrColocationAdvertisementStartInfoMETA{
            buffer: buffer.as_ptr(),
            buffer_size: buffer.len() as _,
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrStartColocationAdvertisementMETA)(
            self.handle,
            &advertisement_info,
            &mut request
        )}.log_error("xrStartColocationAdvertisementMETA");
    }
        
    fn _stop_colocation_advertisement_request(&mut self, xr: &LibOpenXr){
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrStopColocationAdvertisementMETA)(
            self.handle,
            0 as *const _,
            &mut request
        )}.log_error("xrStopColocationAdvertisementMETA");
    }
            
    fn _start_colocation_discovery_request(&mut self, xr: &LibOpenXr){
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrStartColocationDiscoveryMETA)(
            self.handle,
            0 as *const _,
            &mut request
        )}.log_error("xrStartColocationDiscoveryMETA");
    }
        
    fn _start_colocation_discovery_result(&mut self, _xr: &LibOpenXr, response:&XrEventDataColocationDiscoveryResultMETA){
                
        // alright lets parse the buffer
        let buffer = &response.buffer[0..response.buffer_size as usize];
        if let Ok(_data) = AnchorAdvertisement::deserialize_bin(&buffer){
            // alright! we have a discovery result
            //crate::log!("COLOCATED DISCOVERY: {:?} {:?}",response.advertisement_uuid, data.group_uuid);
            //self.query_anchors_request(xr, data.group_uuid)
        }
        else{
            crate::log!("start_colocation_discovery_result deserialize failure");
        }
    }
            
    fn _stop_colocation_discovery_request(&mut self, xr: &LibOpenXr){
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrStopColocationDiscoveryMETA)(
            self.handle,
            0 as *const _,
            &mut request
        )}.log_error("xrStopColocationDiscoveryMETA");              
    }
    
    fn query_local_spaces_request(&mut self, xr: &LibOpenXr, uuids:[XrUuid;2])->Result<XrAsyncRequestIdFB, String>{
        let location_filter_info = XrSpaceStorageLocationFilterInfoFB{
            location: XrSpaceStorageLocationFB::LOCAL,
            ..Default::default()
        };
        /*
        let component_filter_info = XrSpaceComponentFilterInfoFB{
            component_type: XrSpaceComponentTypeFB::STORABLE,
            next: &location_filter_info as *const _ as *const _,
            ..Default::default()
        };*/
        let uuid_filter = XrSpaceUuidFilterInfoFB{
            next: &location_filter_info as *const _ as *const _,
            uuid_count: 2,
            uuids: uuids.as_ptr(),
            ..Default::default()
        };
        let space_query_info = XrSpaceQueryInfoFB{
            query_action: XrSpaceQueryActionFB::LOAD,
            max_result_count: 32,
            filter: &uuid_filter as *const _ as *const _,
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrQuerySpacesFB)(
            self.handle,
            & space_query_info as *const _ as *const _,
            &mut request
        )}.to_result("xrQuerySpacesFB Local")?;
        Ok(request)
    }
    
    fn _query_cloud_spaces_request(&mut self, xr: &LibOpenXr, group_uuid:XrUuid)->Result<XrAsyncRequestIdFB, String>{
        let location_filter_info = XrSpaceStorageLocationFilterInfoFB{
            location: XrSpaceStorageLocationFB::CLOUD,
            ..Default::default()
        };
        let group_filter_info = XrSpaceGroupUuidFilterInfoMETA{
            group_uuid,
            next: &location_filter_info as *const _ as *const _,
            ..Default::default()
        };
        let space_query_info = XrSpaceQueryInfoFB{
            query_action: XrSpaceQueryActionFB::LOAD,
            max_result_count: 32,
            filter: &group_filter_info as *const _ as *const _,
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrQuerySpacesFB)(
            self.handle,
            & space_query_info as *const _ as *const _,
            &mut request
        )}.to_result("xrQuerySpacesFB CLoud")?;
        Ok(request)
    }
    
    fn erase_spaces_request(&mut self, xr: &LibOpenXr, spaces:&[XrSpace])->Result<XrAsyncRequestIdFB, String>{
        let info = XrSpacesEraseInfoMETA{
            spaces: spaces.as_ptr(),
            space_count: spaces.len() as _,
            ..Default::default()
        };
        let mut request = XrAsyncRequestIdFB(0);
        unsafe{(xr.xrEraseSpacesMETA)(
            self.handle,
            & info as *const _ as *const _,
            &mut request
        )}.to_result("xrEraseSpacesMETA")?;
        Ok(request)
    }
        
    fn query_spaces_response(&mut self, xr: &LibOpenXr, response:&XrEventDataSpaceQueryResultsAvailableFB)->Result<(),String>{
        let async_state = self.anchor.async_state.clone();
        match self.anchor.async_state{
            AsyncAnchorState::ClearAndSet{..} | 
            AsyncAnchorState::GetLocalAnchor{..}=>{
                self.anchor.async_state = AsyncAnchorState::Idle;
            }
            _=>{
                self.anchor.async_state.error(true,"query_anchors_response - unexpected")?;
            }
        }
        let mut query = XrSpaceQueryResultsFB::default();
        unsafe{(xr.xrRetrieveSpaceQueryResultsFB)(
            self.handle,
            response.request_id,
            &mut query
        )}.to_result("xrRetrieveSpaceQueryResultsFB")?;
                
        let mut results_vec = Vec::new();
        results_vec.resize(query.result_count_output as usize, XrSpaceQueryResultFB::default());
        let mut query = XrSpaceQueryResultsFB{
            results: results_vec.as_mut_ptr() as *mut _,
            result_capacity_input: results_vec.len() as _,
            result_count_output: results_vec.len() as _,
            ..Default::default()
        };
        unsafe{(xr.xrRetrieveSpaceQueryResultsFB)(
            self.handle,
            response.request_id,
            &mut query
        )}.to_result("xrRetrieveSpaceQueryResultsFB")?;
        
        /*
        for result in &results_vec{
            let components = xr_array_fetch(XrSpaceComponentTypeFB::default(), |cap, len, buf|{
                unsafe{(xr.xrEnumerateSpaceSupportedComponentsFB)(
                    result.space,
                    cap,
                    len, 
                    buf
                )}.to_result("query_anchors_response xrEnumerateSpaceSupportedComponentsFB")
            })?;
            if !components.iter().any(|v| *v == XrSpaceComponentTypeFB::LOCATABLE) {
                crate::log!("query_anchors_response space not LOCATABLE");
            }
            if !components.iter().any(|v| *v == XrSpaceComponentTypeFB::SHARABLE){
                crate::log!("query_anchors_response space not SHARABLE");
            }
            if !components.iter().any(|v| *v == XrSpaceComponentTypeFB::STORABLE){
                crate::log!("query_anchors_response space not STORABLE");
            }
           
            let request = XrSpaceComponentStatusSetInfoFB{
                component_type: XrSpaceComponentTypeFB::LOCATABLE,
                enabled: XrBool32::from_bool(true),
                ..Default::default()
            };
            let mut request_id = XrAsyncRequestIdFB(0);
            unsafe{(xr.xrSetSpaceComponentStatusFB)(
                result.space,
                &request,
                &mut request_id
            )}.log_error("query_anchors_response xrSetSpaceComponentStatusFB LOCATABLE");
            // its shareable
            let request = XrSpaceComponentStatusSetInfoFB{
                component_type: XrSpaceComponentTypeFB::SHARABLE,
                enabled: XrBool32::from_bool(true),
                ..Default::default()
            };
            let mut request_id = XrAsyncRequestIdFB(0);
            unsafe{(xr.xrSetSpaceComponentStatusFB)(
                result.space,
                &request,
                &mut request_id
            )}.log_error("query_anchors_response xrSetSpaceComponentStatusFB SHARABLE");
            let request = XrSpaceComponentStatusSetInfoFB{
                component_type: XrSpaceComponentTypeFB::STORABLE,
                enabled: XrBool32::from_bool(true),
                ..Default::default()
            };
            let mut request_id = XrAsyncRequestIdFB(0);
            unsafe{(xr.xrSetSpaceComponentStatusFB)(
                result.space,
                &request,
                &mut request_id
            )}.log_error("query_anchors_response xrSetSpaceComponentStatusFB STORABLE");
            // alright we have a shared space!
        }*/
        
        match &async_state{
            AsyncAnchorState::ClearAndSet{left_pose, right_pose,..}=>{
                let spaces: Vec<XrSpace> = results_vec.iter().map(|v| v.space).collect();
                self.erase_spaces_request(xr, &spaces).ok();
                // and go on to the next 
                if let Ok(request_id) = self.create_anchor_request(xr, *left_pose){
                    self.anchor.async_state = AsyncAnchorState::LocalAnchorLeft{
                        request_id, 
                        right_pose: *right_pose
                    }
                }
                return Ok(())
            },
            AsyncAnchorState::GetLocalAnchor{left_uuid, right_uuid,..}=>{
                if results_vec.len() != 2{
                    crate::log!("query_anchors_response unexpected result count {}", results_vec.len());
                    return Err("query_anchors_response not zero or 2".to_string());
                }
                let left_index = results_vec.iter().position(|v| v.uuid == *left_uuid).unwrap_or(0);
                let right_index = results_vec.iter().position(|v| v.uuid == *right_uuid).unwrap_or(0);
                let left_space = results_vec[left_index].space;
                let right_space = results_vec[right_index].space;
                if let Err(e) = self.set_space_status_request(xr, &[left_space, right_space], &[
                    XrSpaceComponentTypeFB::STORABLE,
                    XrSpaceComponentTypeFB::LOCATABLE,
                ]){
                    return Err(e);
                }
                crate::log!("Local anchor load completed!");
                self.anchor.anchor = Some(SpaceAnchor{
                    left_space,
                    right_space
                });
                
                return Ok(())
            }
            _=>{}
        }
        Ok(())
        //self.shared_anchor = Some(response.space);
    }
}

#[derive(Default)]
pub struct CxOpenXrAnchor{
    anchor: Option<SpaceAnchor>,
    async_state: AsyncAnchorState,
}

impl CxOpenXrAnchor{
        
    pub fn locate_anchor(&self, xr:&LibOpenXr, local_space:XrSpace, predicted_display_time:XrTime)->Option<XrAnchor>{
        if let Some(anchor) = &self.anchor{
            let left = XrSpaceLocation::locate(xr, local_space, predicted_display_time, anchor.left_space).pose.position;
            let right = XrSpaceLocation::locate(xr, local_space, predicted_display_time, anchor.right_space).pose.position;
            Some(XrAnchor{left, right})
        }
        else{
            None
        }
    }
        
}

struct SpaceAnchor{
    left_space: XrSpace,
    right_space: XrSpace
}

#[allow(unused)]
#[derive(Clone, Debug)]
enum AsyncAnchorState{
    Idle,
    GetLocalAnchor{
        request_id: XrAsyncRequestIdFB,
        left_uuid: XrUuid,
        right_uuid: XrUuid,
    },
    ClearAndSet{
        request_id: XrAsyncRequestIdFB,
        left_pose: Pose,
        right_pose: Pose,
    },
    LocalAnchorLeft{
        request_id: XrAsyncRequestIdFB,
        right_pose: Pose,
    },
    LocalAnchorRight{
        request_id: XrAsyncRequestIdFB,
        left_space: XrSpace,
    }, 
    LocalAnchorSaveSpaces{
        request_id: XrAsyncRequestIdFB,
    },
    LocalAnchorShareSpaces{
        request_id: XrAsyncRequestIdFB,
    }
}
impl AsyncAnchorState{
    fn error(&mut self, cond:bool, msg:&str)->Result<(),String>{
        if cond{
            crate::log!("AsyncAnchorState error {msg} {:?}", self);
            *self = AsyncAnchorState::Idle;
            return Err("AsyncAnchorState condition fail".into())
        }
        Ok(())
    }
    
    fn result(&mut self, result:XrResult)->Result<(),String>{
        if result != XrResult::SUCCESS{
            crate::log!("AsyncAnchorState error {:?} {:?}", result, self);
            *self = AsyncAnchorState::Idle;
            return Err("AsyncAnchorState result not success".into())
        }
        Ok(())
    }
    
    fn idle(&self)->bool{
         if let AsyncAnchorState::Idle = self{true}else{false}
    }
}

impl Default for AsyncAnchorState{
    fn default()->Self{Self::Idle}
}


#[derive(SerBin, DeBin)]
struct AnchorAdvertisement{
    group_uuid: XrUuid,
    anchor_left_uuid: XrUuid,
    anchor_right_uuid: XrUuid,
}
