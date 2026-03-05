use std::io::{self, Read, Cursor};

// Binary reader helpers
trait ReadExt: Read {
    fn read_i32(&mut self) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(i32::from_le_bytes(buf))
    }
    fn read_u8_val(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }
    fn read_u16(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }
    fn read_f32(&mut self) -> io::Result<f32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(f32::from_le_bytes(buf))
    }
    fn read_f64(&mut self) -> io::Result<f64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(f64::from_le_bytes(buf))
    }
    fn skip(&mut self, n: usize) -> io::Result<()> {
        let mut buf = vec![0u8; n];
        self.read_exact(&mut buf)
    }
}
impl<R: Read> ReadExt for R {}

#[derive(Debug, Clone)]
pub struct HAFormula {
    pub iteration_count: i32,
    pub formula_nr: i32,
    pub option_count: i32,
    pub custom_name: String,
    pub option_types: [u8; 16],
    pub option_values: [f64; 16],
}

#[derive(Debug, Clone)]
pub struct HeaderCustomAddon {
    pub version: u8,
    pub b_options1: u8,
    pub b_hyb_opt1: u8,
    pub b_options2: u8,
    pub b_options3: u8,
    pub b_hyb_opt2: u16,
    pub formula_count: u8,
    pub formulas: [HAFormula; 6],
}

#[derive(Debug, Clone)]
pub struct M3PLight {
    pub color: [u8; 3],
    pub lamp: f64,
    pub angle_xy: f64,
    pub angle_z: f64,
    pub l_option: u8,
    pub l_function: u8,
    pub l_amp: f64,
    pub additional_byte_ex: u8,
    pub free_byte: u8,
}

#[derive(Debug, Clone)]
pub struct M3PLCol {
    pub pos: u16,
    pub color_dif: [u8; 4],
    pub color_spe: [u8; 4],
}

#[derive(Debug, Clone)]
pub struct M3PICol {
    pub pos: u16,
    pub color: [u8; 4],
}

#[derive(Debug, Clone)]
pub struct M3PLighting {
    pub roughness_factor: u8,
    pub additional_options: u8,
    pub calc_pix_col_sqr: bool,
    pub dyn_fog_col: [u8; 3],
    pub dyn_fog_col2: [u8; 3],
    pub ambient_bottom: [u8; 3],
    pub ambient_top: [u8; 3],
    pub depth_col: [u8; 3],
    pub depth_col2: [u8; 3],
    pub s_depth: f64,
    pub tbpos_3: i32,
    pub tbpos_5: i32,
    pub tbpos_6: i32,
    pub tbpos_7: i32,
    pub tbpos_9: i32,
    pub tbpos_10: i32,
    pub tbpos_11: i32,
    pub tboptions: u32,
    pub fine_col_adj_1: u8,
    pub fine_col_adj_2: u8,
    pub lights: Vec<M3PLight>,
    pub l_cols: Vec<M3PLCol>,
    pub i_cols: Vec<M3PICol>,
}

#[derive(Debug, Clone)]
pub struct M3PFile {
    pub mand_id: i32,
    pub width: i32,
    pub height: i32,
    pub iterations: i32,
    pub min_iterations: i32,
    pub i_options: u16,
    pub b_new_options: u8,
    pub b_dfog_it: u8,
    pub b_color_on_it: u8,
    pub b_vol_light_nr: u8,
    pub b_calculate_hard_shadow: u8,
    pub b_hs_calculated: u8,
    pub b_calc1_hs_soft: u8,
    pub z_start: f64,
    pub z_end: f64,
    pub x_mid: f64,
    pub y_mid: f64,
    pub z_mid: f64,
    pub xw_rot: f64,
    pub yw_rot: f64,
    pub zw_rot: f64,
    pub zoom: f64,
    pub rstop: f64,
    pub fov_y: f64,
    pub step_width: f64,
    pub s_raystep_limiter: f32,
    pub de_stop: f32,
    pub b_vary_de_stop: bool,
    pub z_step_div: f32,
    pub soft_shadow_radius: f64,
    pub hs_max_length_multiplier: f64,
    pub is_julia: bool,
    pub julia_x: f64,
    pub julia_y: f64,
    pub julia_z: f64,
    pub julia_w: f64,
    pub view_matrix: [[f64; 3]; 3],
    pub lighting: M3PLighting,
    pub ssao: M3PSSAO,
    pub addon: HeaderCustomAddon,
    // Raw bytes for anything we haven't parsed yet
    pub raw_header: Vec<u8>,
}

impl HAFormula {
    fn read(cursor: &mut Cursor<&[u8]>) -> io::Result<Self> {
        let iteration_count = cursor.read_i32()?;
        let formula_nr = cursor.read_i32()?;
        let option_count = cursor.read_i32()?;

        let mut name_buf = [0u8; 32];
        cursor.read_exact(&mut name_buf)?;
        let name_len = name_buf.iter().position(|&b| b == 0).unwrap_or(32);
        let custom_name = String::from_utf8_lossy(&name_buf[..name_len]).to_string();

        let mut option_types = [0u8; 16];
        cursor.read_exact(&mut option_types)?;

        let mut option_values = [0.0f64; 16];
        // 60 bytes read so far (4+4+4+32+16), values start here
        // But the struct is 188 bytes: 12 + 32 + 16 + 128 = 188
        // option_values: 16 doubles = 128 bytes
        for i in 0..16 {
            option_values[i] = cursor.read_f64()?;
        }

        Ok(HAFormula {
            iteration_count,
            formula_nr,
            option_count,
            custom_name,
            option_types,
            option_values,
        })
    }
}

fn short_float_to_f64(v: u16) -> f64 {
    // MB3D ShortFloat: mantissa/exponent packed as signed bytes.
    // value = mantissa * 10^(exp-1), exponent clamped to [-25, 25].
    let bytes = v.to_le_bytes();
    let mant = i8::from_le_bytes([bytes[0]]) as f64;
    let exp = i8::from_le_bytes([bytes[1]]) as i32;
    let exp_clamped = exp.clamp(-25, 25) - 1;
    mant * 10f64.powi(exp_clamped)
}

pub fn parse(path: &str) -> io::Result<M3PFile> {
    let data = std::fs::read(path)?;
    let mut c = Cursor::new(data.as_slice());

    let mand_id = c.read_i32()?;
    if mand_id != 44 {
        return Err(io::Error::new(io::ErrorKind::InvalidData,
            format!("Invalid MandId: {} (expected 44)", mand_id)));
    }

    let width = c.read_i32()?;
    let height = c.read_i32()?;
    let iterations = c.read_i32()?;

    // offset 16: iOptions (Word=u16), bNewOptions (Byte), bColorOnIt (Byte)
    let i_options = c.read_u16()?;
    let b_new_options = c.read_u8_val()?;
    let b_color_on_it = c.read_u8_val()?;

    // offset 20: dZstart, dZend (2 doubles = 16 bytes)
    let z_start = c.read_f64()?;
    let z_end = c.read_f64()?;

    // offset 36: dXmid, dYmid, dZmid (3 doubles = 24 bytes)
    let x_mid = c.read_f64()?;
    let y_mid = c.read_f64()?;
    let z_mid = c.read_f64()?;

    // offset 60: dXWrot, dYWrot, dZWrot (3 doubles = 24 bytes)
    let xw_rot = c.read_f64()?;
    let yw_rot = c.read_f64()?;
    let zw_rot = c.read_f64()?;

    // offset 84: dZoom, RStop (2 doubles = 16 bytes)
    let zoom = c.read_f64()?;
    let rstop = c.read_f64()?;

    // offset 100: skip to offset 108 (iReflectsCalcTime=i32 + 4 bytes padding)
    c.skip(8)?;

    // offset 108: dFOVy (double)
    let fov_y = c.read_f64()?;

    // offset 116: skip to offset 154 (dStepWidth)
    c.skip(38)?;

    // offset 154: dStepWidth (double)
    let step_width = c.read_f64()?;

    // offset 162: skip to 177 (bVaryDEstopOnFOV=byte at 162, then skip to 177)
    c.skip(15)?;

    // offset 177: sDEstop (Single = f32)
    let de_stop = c.read_f32()?;

    // offset 181: skip 1 byte to 182
    c.skip(1)?;

    // offset 182: mZstepDiv (Single = f32)
    let z_step_div = c.read_f32()?;

    // offset 186: skip to 223 for bDFogIt
    c.skip(37)?; // 186 + 37 = 223
    let b_dfog_it = c.read_u8_val()?; // 223

    // TMandHeader10:
    //   MCSoftShadowRadius (ShortFloat) @224
    //   HSmaxLengthMultiplier (Single) @226
    let soft_shadow_radius = short_float_to_f64(
        u16::from_le_bytes(data[224..226].try_into().unwrap())
    );
    let hs_max_length_multiplier = f32::from_le_bytes(data[226..230].try_into().unwrap()) as f64;
    let s_raystep_limiter = f32::from_le_bytes(data[242..246].try_into().unwrap());

    // skip to 246 for view matrix
    c.skip(22)?; // 224 + 22 = 246
    
    // bVolLightNr is at 343
    let b_vol_light_nr = data[343];
    let b_calculate_hard_shadow = data[133];
    let b_calc1_hs_soft = data[139];
    let b_hs_calculated = data[163];

    // offset 246: hVGrads 3x3 matrix of doubles (72 bytes)
    let mut view_matrix = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            view_matrix[i][j] = c.read_f64()?;
        }
    }

    // Keep raw header bytes for parity/debug.
    let raw_header = data.clone();

    // TMandHeader10:
    //   bIsJulia @190, dJx/dJy/dJz/dJw @191..222
    let is_julia = data[190] != 0;
    let julia_x = f64::from_le_bytes(data[191..199].try_into().unwrap());
    let julia_y = f64::from_le_bytes(data[199..207].try_into().unwrap());
    let julia_z = f64::from_le_bytes(data[207..215].try_into().unwrap());
    let julia_w = f64::from_le_bytes(data[215..223].try_into().unwrap());

    let var_col_zpos = i16::from_le_bytes(data[432..434].try_into().unwrap());
    let roughness_factor = data[434];
    let _b_color_map = data[435];
    let additional_options = data[439];
    // TLightingParas9 stores TBpos[3..11] starting at offset 440.
    let tbpos_3 = i32::from_le_bytes(data[440..444].try_into().unwrap());  // TBpos[3]
    let tbpos_4 = i32::from_le_bytes(data[444..448].try_into().unwrap());  // TBpos[4]
    let tbpos_5 = i32::from_le_bytes(data[448..452].try_into().unwrap());  // TBpos[5]
    let tbpos_6 = i32::from_le_bytes(data[452..456].try_into().unwrap());  // TBpos[6]
    let tbpos_7 = i32::from_le_bytes(data[456..460].try_into().unwrap());  // TBpos[7]
    let tboptions = u32::from_le_bytes(data[476..480].try_into().unwrap());
    let tbpos_9 = i32::from_le_bytes(data[464..468].try_into().unwrap());
    let tbpos_10 = i32::from_le_bytes(data[468..472].try_into().unwrap());
    let tbpos_11 = i32::from_le_bytes(data[472..476].try_into().unwrap());
    let fine_col_adj_1 = data[480];
    let fine_col_adj_2 = data[481];
    let _l_version = (tboptions >> 21) & 7;
    let s_depth = tbpos_4 as f64 * 0.8e-6;
    println!("  s_depth: {} (tbpos_4: {}), var_col_zpos: {}, roughness_factor: {}, tbpos_3: {}, tbpos_5: {}, tbpos_6: {}, tbpos_7: {}, bColCycling: {}",
        s_depth, tbpos_4, var_col_zpos, roughness_factor, tbpos_3, tbpos_5, tbpos_6, tbpos_7, (tboptions & 0x4000) != 0);
    println!("  tbpos_9: {}, tbpos_10: {}, tboptions: {:08x}, fine_col_adj_1: {}, fine_col_adj_2: {}", tbpos_9, tbpos_10, tboptions, fine_col_adj_1, fine_col_adj_2);
    println!("  depth_col: {:?}, depth_col2: {:?}", [data[492], data[493], data[494]], [data[496], data[497], data[498]]);

    // Parse lighting at fixed offsets
    let mut lighting = M3PLighting {
        roughness_factor,
        additional_options,
        calc_pix_col_sqr: (additional_options & 1) != 0,
        dyn_fog_col: [data[487], data[491], data[495]],
        dyn_fog_col2: [data[436], data[437], data[438]],
        ambient_bottom: [data[484], data[485], data[486]],
        ambient_top: [data[488], data[489], data[490]],
        depth_col: [data[492], data[493], data[494]],
        depth_col2: [data[496], data[497], data[498]],
        s_depth,
        tbpos_3,
        tbpos_5,
        tbpos_6,
        tbpos_7,
        tbpos_9,
        tbpos_10,
        tbpos_11,
        tboptions,
        fine_col_adj_1,
        fine_col_adj_2,
        lights: Vec::new(),
        l_cols: Vec::new(),
        i_cols: Vec::new(),
    };

    // 6 lights (6 * 32 bytes) starting at 500
    for i in 0..6 {
        let offset = 500 + i * 32;
        let mut lc = Cursor::new(&data[offset..offset + 32]);
        let l_option = lc.read_u8_val()?;
        let l_function = lc.read_u8_val()?;

        let lamp_word = lc.read_u16()?;
        let lamp = short_float_to_f64(lamp_word);

        let r = lc.read_u8_val()?;
        let g = lc.read_u8_val()?;
        let b = lc.read_u8_val()?;
        let _light_map_nr = lc.read_u16()?;
        
        // Double7B for LXpos
        let mut x_bytes = [0u8; 8];
        lc.read_exact(&mut x_bytes[1..8])?;
        let angle_xy = f64::from_le_bytes(x_bytes);

        // TLight8 layout (after LXpos):
        // AdditionalByteEx, LYpos(Double7B), FreeByte, LZpos(Double7B)
        let additional_byte_ex = lc.read_u8_val()?; // AdditionalByteEx

        // Double7B for LYpos
        let mut y_bytes = [0u8; 8];
        lc.read_exact(&mut y_bytes[1..8])?;
        let angle_z = f64::from_le_bytes(y_bytes);

        let free_byte = lc.read_u8_val()?; // FreeByte

        // Double7B for LZpos (currently unused in the renderer)
        let mut _z_bytes = [0u8; 8];
        lc.read_exact(&mut _z_bytes[1..8])?;

        // Keep a minimum amplitude for robust rendering if the file has zeroed legacy values.
        let l_amp = if lamp.abs() < 1.0e-20 { 1.0 } else { lamp };

        println!(
            "  Light {}: color=[{}, {}, {}], option={}, function={}, lamp={:.6e}, angle_xy={}, angle_z={}, add_ex={}, free={}",
            i, r, g, b, l_option, l_function, l_amp, angle_xy, angle_z, additional_byte_ex, free_byte
        );

        lighting.lights.push(M3PLight {
            color: [r, g, b],
            lamp: l_amp,
            angle_xy,
            angle_z,
            l_option,
            l_function,
            l_amp,
            additional_byte_ex,
            free_byte,
        });
    }

    // LCols starting at 692
    for i in 0..10 {
        let offset = 692 + i * 10;
        let mut gc = Cursor::new(&data[offset..offset + 10]);
        let pos = gc.read_u16()?;
        let mut color_dif = [0u8; 4];
        gc.read_exact(&mut color_dif)?;
        let mut color_spe = [0u8; 4];
        gc.read_exact(&mut color_spe)?;
        lighting.l_cols.push(M3PLCol { pos, color_dif, color_spe });
    }

    // ICols starting at 792
    for i in 0..4 {
        let offset = 792 + i * 6;
        let mut gc = Cursor::new(&data[offset..offset + 6]);
        let pos = gc.read_u16()?;
        let mut color = [0u8; 4];
        gc.read_exact(&mut color)?;
        lighting.i_cols.push(M3PICol { pos, color });
    }

    // Parse HeaderCustomAddon (starts at offset 840)
    let addon_offset = 840;
    let mut ac = Cursor::new(&data[addon_offset..]);

    let version = ac.read_u8_val()?;
    let b_options1 = ac.read_u8_val()?;
    let b_options2 = ac.read_u8_val()?;
    println!("  b_options2: {}", b_options2);
    let b_options3 = ac.read_u8_val()?;
    let formula_count = ac.read_u8_val()?;
    let b_hyb_opt1 = ac.read_u8_val()?;
    let b_hyb_opt2 = ac.read_u16()?;

    let mut formulas = Vec::new();
    for _ in 0..6 {
        formulas.push(HAFormula::read(&mut ac)?);
    }

    let addon = HeaderCustomAddon {
        version,
        b_options1,
        b_hyb_opt1,
        b_options2,
        b_options3,
        b_hyb_opt2,
        formula_count,
        formulas: [
            formulas[0].clone(),
            formulas[1].clone(),
            formulas[2].clone(),
            formulas[3].clone(),
            formulas[4].clone(),
            formulas[5].clone(),
        ],
    };

    let b_vary_de_stop = data[162] != 0;
    println!("  bVaryDEstop: {}", b_vary_de_stop);
    println!("  sRaystepLimiter: {:.6}", s_raystep_limiter);
    println!(
        "  hard shadow flags: bCalculateHardShadow=0x{b_calculate_hard_shadow:02x}, bCalc1HSsoft=0x{b_calc1_hs_soft:02x}, bHScalculated=0x{b_hs_calculated:02x}"
    );
    let b_calc_amb_shadow_automatic = data[149];
    println!("  b_calc_amb_shadow_automatic: {}", b_calc_amb_shadow_automatic);
    let calc_amb_shadow = (b_calc_amb_shadow_automatic & 1) != 0;
    let quality = ((b_calc_amb_shadow_automatic >> 4) & 3) as i32;
    let ssao_r_count = data[187] as i32;
    let ao_dithering = data[188] as i32;
    let deao_max_l = f32::from_le_bytes(data[374..378].try_into().unwrap()) as f64;
    
    let amb_shad = (lighting.tbpos_11 & 0xFF) as f64 / 53.0;
    
    let diffuse_shadowing = lighting
        .lights
        .get(3)
        .map(|l| l.additional_byte_ex as f64 / 256.0)
        .unwrap_or(data[600 + 16] as f64 / 256.0);

    let ssao = M3PSSAO {
        quality,
        deao_max_l,
        ssao_r_count,
        ao_dithering,
        calc_amb_shadow,
        diffuse_shadowing,
        amb_shad,
    };

        println!("  SSAO: enabled={}, quality={}, rays={}, maxL={}, amb_shad={}, diff_shad={}", 
            ssao.calc_amb_shadow, ssao.quality, ssao.ssao_r_count, ssao.deao_max_l, ssao.amb_shad, ssao.diffuse_shadowing);
        
        println!("LCols:");
        for (i, c) in lighting.l_cols.iter().enumerate() {
            println!("  {}: pos={}, dif={:?}, spe={:?}", i, c.pos, c.color_dif, c.color_spe);
        }
        
        let min_iterations = i32::from_le_bytes(data[135..139].try_into().unwrap());

    Ok(M3PFile {
        mand_id, width, height, iterations, min_iterations,
        i_options, b_new_options, b_color_on_it, b_dfog_it, b_vol_light_nr,
        b_calculate_hard_shadow, b_hs_calculated, b_calc1_hs_soft,
        z_start, z_end,
        x_mid, y_mid, z_mid,
        xw_rot, yw_rot, zw_rot,
        zoom, rstop, fov_y, step_width,
        s_raystep_limiter,
        b_vary_de_stop,
        de_stop, z_step_div,
        soft_shadow_radius, hs_max_length_multiplier,
        is_julia, julia_x, julia_y, julia_z, julia_w,
        view_matrix,
        lighting,
        ssao,
        addon,
        raw_header,
    })
}

#[derive(Debug, Clone)]
pub struct M3PSSAO {
    pub quality: i32,
    pub deao_max_l: f64,
    pub ssao_r_count: i32,
    pub ao_dithering: i32,
    pub calc_amb_shadow: bool,
    pub diffuse_shadowing: f64,
    pub amb_shad: f64,
}
