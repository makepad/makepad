use crate::cx::*;
use crate::events::*;
use std::io::prelude::*;
use std::fs::File;
use crate::math::*;

impl Cx{
    pub fn map_winit_event(&mut self, winit_event:winit::Event, glutin_window:&winit::Window)->Event{
        match winit_event{
            winit::Event::WindowEvent{ event, .. } => match event {
                winit::WindowEvent::CloseRequested =>{
                    self.running = false;
                    return Event::CloseRequested
                },
                winit::WindowEvent::Resized(logical_size) => {
                    let dpi_factor = glutin_window.get_hidpi_factor();
                    let old_dpi_factor = self.turtle.target_dpi_factor as f32;
                    let old_size = self.turtle.target_size.clone();
                    self.turtle.target_dpi_factor = dpi_factor as f32;
                    self.turtle.target_size = vec2(logical_size.width as f32, logical_size.height as f32);
                    return Event::Resized(ResizedEvent{
                        old_size: old_size,
                        old_dpi_factor: old_dpi_factor,
                        new_size: self.turtle.target_size.clone(),
                        new_dpi_factor: self.turtle.target_dpi_factor
                    })
                },
                _ => ()
            },
            _ => ()
        }
        Event::None
    }

    pub fn load_binary_deps_from_file(&mut self){
        for i in 0..self.fonts.font_resources.len(){
            let resource_name = &self.fonts.font_resources[i].name;
            // lets turn a file into a binary dep
            let file_result = File::open(&resource_name);
            if let Ok(mut file) = file_result{
                let mut buffer = Vec::new();
                // read the whole file
                if file.read_to_end(&mut buffer).is_ok(){
                    // store it in a bindep
                    let mut bin_dep = BinaryDep::new_from_vec(resource_name.clone(), &buffer);
                    let _err = self.fonts.load_from_binary_dep(&mut bin_dep, &mut self.textures);

                    //     println!("Error loading font {} ", resource_name);
                    //};
                }
            }
            else{
                println!("Error loading font {} ", resource_name);
            }
        }
    }

    pub fn log(val:&str){
        println!("{}",val);
    }
}