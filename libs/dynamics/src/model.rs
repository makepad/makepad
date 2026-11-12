//! Model storage, allocation and defaults.
//!
//! Translated from `mjmodel.h`, `mjxmacro.h`, `src/engine/engine_io.c` and `engine_init.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::types::*;

// mjVisual: global 52 + quality 20 + headlight 40 + map 52 + scale 68
// + rgba 400 = 632 bytes. All members have 4-byte size/alignment; no padding.
pub const VISUAL_BYTES: usize = 632;

/// MJMODEL_SIZES and MJMODEL_POINTERS, expanded in the original sub-list order.
/// Exported for table-driven model tooling; the callback receives sizes and arrays.
#[doc(hidden)]
#[macro_export]
macro_rules! model_fields {
    ($callback:ident ($($context:tt)*)) => {
        $callback! { ($($context)*)
            sizes {
                nq,
                nv,
                nu,
                nactuator,
                nout,
                na,
                nbody,
                nbvh,
                nbvhstatic,
                nbvhdynamic,
                noct,
                njnt,
                ntree,
                n_m,
                n_b,
                n_c,
                n_d,
                ngeom,
                nsite,
                ncam,
                nlight,
                nflex,
                nflexnode,
                nflexvert,
                nflexedge,
                nflexelem,
                nflexelemdata,
                nflexstiffness,
                nflexbending,
                nefm0dof,
                nefm0l,
                nefm_cvert,
                nflexelemedge,
                nflexshelldata,
                nflextexcoord,
                n_jfe,
                nmesh,
                nmeshvert,
                nmeshnormal,
                nmeshtexcoord,
                nmeshface,
                nmeshgraph,
                nmeshpoly,
                nmeshpolyvert,
                nmeshpolymap,
                nskin,
                nskinvert,
                nskintexvert,
                nskinface,
                nskinbone,
                nskinbonevert,
                nhfield,
                nhfielddata,
                ntex,
                ntexdata,
                nmat,
                npair,
                nexclude,
                neq,
                ntendon,
                n_jten,
                nwrap,
                nsensor,
                nnumeric,
                nnumericdata,
                ntext,
                ntextdata,
                ntuple,
                ntupledata,
                nkey,
                nmocap,
                nplugin,
                npluginattr,
                nuser_body,
                nuser_jnt,
                nuser_geom,
                nuser_site,
                nuser_cam,
                nuser_tendon,
                nuser_actuator,
                nuser_sensor,
                nnames,
                npaths,
                nplastic,
                nnames_map,
                n_jmom,
                ngravcomp,
                nemax,
                njmax,
                nconmax,
                npolygonmax,
                nmeshdegmax,
                nuserdata,
                nsensordata,
                npluginstate,
                nhistory,
                narena,
                nbuffer,
            }
            arrays {
                qpos0: f64 [nq, 1],
                qpos_spring: f64 [nq, 1],
                body_parentid: i32 [nbody, 1],
                body_rootid: i32 [nbody, 1],
                body_weldid: i32 [nbody, 1],
                body_mocapid: i32 [nbody, 1],
                body_jntnum: i32 [nbody, 1],
                body_jntadr: i32 [nbody, 1],
                body_dofnum: i32 [nbody, 1],
                body_dofadr: i32 [nbody, 1],
                body_treeid: i32 [nbody, 1],
                body_geomnum: i32 [nbody, 1],
                body_geomadr: i32 [nbody, 1],
                body_simple: u8 [nbody, 1],
                body_sameframe: u8 [nbody, 1],
                body_pos: f64 [nbody, 3],
                body_quat: f64 [nbody, 4],
                body_ipos: f64 [nbody, 3],
                body_iquat: f64 [nbody, 4],
                body_mass: f64 [nbody, 1],
                body_subtreemass: f64 [nbody, 1],
                body_inertia: f64 [nbody, 3],
                body_invweight0: f64 [nbody, 2],
                body_gravcomp: f64 [nbody, 1],
                body_margin: f64 [nbody, 1],
                body_user: f64 [nbody, nuser_body],
                body_plugin: i32 [nbody, 1],
                body_contype: i32 [nbody, 1],
                body_conaffinity: i32 [nbody, 1],
                body_bvhadr: i32 [nbody, 1],
                body_bvhnum: i32 [nbody, 1],
                bvh_depth: i32 [nbvh, 1],
                bvh_child: i32 [nbvh, 2],
                bvh_nodeid: i32 [nbvh, 1],
                bvh_aabb: f64 [nbvhstatic, 6],
                oct_depth: i32 [noct, 1],
                oct_child: i32 [noct, 8],
                oct_aabb: f64 [noct, 6],
                oct_coeff: f64 [noct, 8],
                jnt_type: i32 [njnt, 1],
                jnt_qposadr: i32 [njnt, 1],
                jnt_dofadr: i32 [njnt, 1],
                jnt_bodyid: i32 [njnt, 1],
                jnt_actuatorid: i32 [njnt, 1],
                jnt_group: i32 [njnt, 1],
                jnt_plasticadr: i32 [njnt, 1],
                jnt_limited: u8 [njnt, 1],
                jnt_actfrclimited: u8 [njnt, 1],
                jnt_actgravcomp: u8 [njnt, 1],
                jnt_solref: f64 [njnt, NREF],
                jnt_solimp: f64 [njnt, NIMP],
                jnt_pos: f64 [njnt, 3],
                jnt_axis: f64 [njnt, 3],
                jnt_stiffness: f64 [njnt, 1],
                jnt_stiffnesspoly: f64 [njnt, NPOLY],
                jnt_springyield: f64 [njnt, 1],
                jnt_range: f64 [njnt, 2],
                jnt_actfrcrange: f64 [njnt, 2],
                jnt_margin: f64 [njnt, 1],
                jnt_user: f64 [njnt, nuser_jnt],
                dof_bodyid: i32 [nv, 1],
                dof_jntid: i32 [nv, 1],
                dof_parentid: i32 [nv, 1],
                dof_treeid: i32 [nv, 1],
                dof_madr: i32 [nv, 1],
                dof_simplenum: i32 [nv, 1],
                dof_solref: f64 [nv, NREF],
                dof_solimp: f64 [nv, NIMP],
                dof_frictionloss: f64 [nv, 1],
                dof_armature: f64 [nv, 1],
                dof_damping: f64 [nv, 1],
                dof_dampingpoly: f64 [nv, NPOLY],
                dof_invweight0: f64 [nv, 1],
                dof_m0: f64 [nv, 1],
                dof_length: f64 [nv, 1],
                tree_bodyadr: i32 [ntree, 1],
                tree_bodynum: i32 [ntree, 1],
                tree_dofadr: i32 [ntree, 1],
                tree_dofnum: i32 [ntree, 1],
                tree_sleep_policy: i32 [ntree, 1],
                geom_type: i32 [ngeom, 1],
                geom_contype: i32 [ngeom, 1],
                geom_conaffinity: i32 [ngeom, 1],
                geom_condim: i32 [ngeom, 1],
                geom_bodyid: i32 [ngeom, 1],
                geom_dataid: i32 [ngeom, 1],
                geom_matid: i32 [ngeom, 1],
                geom_group: i32 [ngeom, 1],
                geom_priority: i32 [ngeom, 1],
                geom_plugin: i32 [ngeom, 1],
                geom_sameframe: u8 [ngeom, 1],
                geom_castshadow: u8 [ngeom, 1],
                geom_solmix: f64 [ngeom, 1],
                geom_solref: f64 [ngeom, NREF],
                geom_solimp: f64 [ngeom, NIMP],
                geom_size: f64 [ngeom, 3],
                geom_aabb: f64 [ngeom, 6],
                geom_rbound: f64 [ngeom, 1],
                geom_pos: f64 [ngeom, 3],
                geom_quat: f64 [ngeom, 4],
                geom_friction: f64 [ngeom, 3],
                geom_margin: f64 [ngeom, 1],
                geom_gap: f64 [ngeom, 1],
                geom_surfacevel: f64 [ngeom, 6],
                geom_adhesion: f64 [ngeom, 1],
                geom_fluid: f64 [ngeom, NFLUID],
                geom_user: f64 [ngeom, nuser_geom],
                geom_rgba: f32 [ngeom, 4],
                site_type: i32 [nsite, 1],
                site_bodyid: i32 [nsite, 1],
                site_dataid: i32 [nsite, 1],
                site_matid: i32 [nsite, 1],
                site_group: i32 [nsite, 1],
                site_sameframe: u8 [nsite, 1],
                site_size: f64 [nsite, 3],
                site_pos: f64 [nsite, 3],
                site_quat: f64 [nsite, 4],
                site_user: f64 [nsite, nuser_site],
                site_rgba: f32 [nsite, 4],
                cam_mode: i32 [ncam, 1],
                cam_bodyid: i32 [ncam, 1],
                cam_targetbodyid: i32 [ncam, 1],
                cam_pos: f64 [ncam, 3],
                cam_quat: f64 [ncam, 4],
                cam_poscom0: f64 [ncam, 3],
                cam_pos0: f64 [ncam, 3],
                cam_mat0: f64 [ncam, 9],
                cam_projection: i32 [ncam, 1],
                cam_fovy: f64 [ncam, 1],
                cam_ipd: f64 [ncam, 1],
                cam_resolution: i32 [ncam, 2],
                cam_output: i32 [ncam, 1],
                cam_sensorsize: f32 [ncam, 2],
                cam_intrinsic: f32 [ncam, 4],
                cam_user: f64 [ncam, nuser_cam],
                light_mode: i32 [nlight, 1],
                light_bodyid: i32 [nlight, 1],
                light_targetbodyid: i32 [nlight, 1],
                light_type: i32 [nlight, 1],
                light_texid: i32 [nlight, 1],
                light_castshadow: u8 [nlight, 1],
                light_bulbradius: f32 [nlight, 1],
                light_intensity: f32 [nlight, 1],
                light_range: f32 [nlight, 1],
                light_active: u8 [nlight, 1],
                light_pos: f64 [nlight, 3],
                light_dir: f64 [nlight, 3],
                light_poscom0: f64 [nlight, 3],
                light_pos0: f64 [nlight, 3],
                light_dir0: f64 [nlight, 3],
                light_attenuation: f32 [nlight, 3],
                light_cutoff: f32 [nlight, 1],
                light_softness: f32 [nlight, 1],
                light_exponent: f32 [nlight, 1],
                light_ambient: f32 [nlight, 3],
                light_diffuse: f32 [nlight, 3],
                light_specular: f32 [nlight, 3],
                flex_contype: i32 [nflex, 1],
                flex_conaffinity: i32 [nflex, 1],
                flex_condim: i32 [nflex, 1],
                flex_priority: i32 [nflex, 1],
                flex_solmix: f64 [nflex, 1],
                flex_solref: f64 [nflex, NREF],
                flex_solimp: f64 [nflex, NIMP],
                flex_friction: f64 [nflex, 3],
                flex_margin: f64 [nflex, 1],
                flex_gap: f64 [nflex, 1],
                flex_selfcollide: i32 [nflex, 1],
                flex_activelayers: i32 [nflex, 1],
                flex_passive: i32 [nflex, 1],
                flex_dim: i32 [nflex, 1],
                flex_matid: i32 [nflexelem, 1],
                flex_group: i32 [nflex, 1],
                flex_interp: i32 [nflex, 1],
                flex_cellnum: i32 [nflex, 3],
                flex_nodeadr: i32 [nflex, 1],
                flex_nodenum: i32 [nflex, 1],
                flex_vertadr: i32 [nflex, 1],
                flex_vertnum: i32 [nflex, 1],
                flex_edgeadr: i32 [nflex, 1],
                flex_edgenum: i32 [nflex, 1],
                flex_elemadr: i32 [nflex, 1],
                flex_elemnum: i32 [nflex, 1],
                flex_elemdataadr: i32 [nflex, 1],
                flex_stiffnessadr: i32 [nflex, 1],
                flex_elemedgeadr: i32 [nflex, 1],
                flex_bendingadr: i32 [nflex, 1],
                flex_plasticadr: i32 [nflex, 1],
                flex_plasticmode: i32 [nflex, 1],
                flex_shellnum: i32 [nflex, 1],
                flex_shelldataadr: i32 [nflex, 1],
                flex_texcoordadr: i32 [nflex, 1],
                flex_nodebodyid: i32 [nflexnode, 1],
                flex_vertbodyid: i32 [nflexvert, 1],
                flex_vertcell: i32 [nflexvert, 1],
                flex_edge: i32 [nflexedge, 2],
                flex_edgeflap: i32 [nflexedge, 2],
                flex_elem: i32 [nflexelemdata, 1],
                flex_elemtexcoord: i32 [nflexelemdata, 1],
                flex_elemedge: i32 [nflexelemedge, 1],
                flex_elemlayer: i32 [nflexelem, 1],
                flex_shell: i32 [nflexshelldata, 1],
                flex_vert: f64 [nflexvert, 3],
                flex_vert0: f64 [nflexvert, 3],
                flex_node: f64 [nflexnode, 3],
                flex_node0: f64 [nflexnode, 3],
                flexedge_length0: f64 [nflexedge, 1],
                flexedge_invweight0: f64 [nflexedge, 1],
                flex_radius: f64 [nflex, 1],
                flex_size: f64 [nflex, 3],
                flex_stiffness: f64 [nflexstiffness, 1],
                flex_bending: f64 [nflexbending, 1],
                efm0_dofid: i32 [nefm0dof, 1],
                efm0_l_rownnz: i32 [nefm0dof, 1],
                efm0_l_rowadr: i32 [nefm0dof, 1],
                efm0_l_colind: i32 [nefm0l, 1],
                efm0_l: f64 [nefm0l, 1],
                efm_c_perm: i32 [nefm_cvert, 1],
                flex_damping: f64 [nflex, 1],
                flex_yield: f64 [nflex, 2],
                flex_edgestiffness: f64 [nflex, 1],
                flex_edgedamping: f64 [nflex, 1],
                flex_edgeequality: i32 [nflex, 1],
                flex_rigid: u8 [nflex, 1],
                flexedge_rigid: u8 [nflexedge, 1],
                flex_centered: u8 [nflex, 1],
                flex_flatskin: u8 [nflex, 1],
                flex_bvhadr: i32 [nflex, 1],
                flex_bvhnum: i32 [nflex, 1],
                flexedge_j_rownnz: i32 [nflexedge, 1],
                flexedge_j_rowadr: i32 [nflexedge, 1],
                flexedge_j_colind: i32 [n_jfe, 1],
                flex_rgba: f32 [nflex, 4],
                flex_texcoord: f32 [nflextexcoord, 2],
                mesh_vertadr: i32 [nmesh, 1],
                mesh_vertnum: i32 [nmesh, 1],
                mesh_faceadr: i32 [nmesh, 1],
                mesh_facenum: i32 [nmesh, 1],
                mesh_bvhadr: i32 [nmesh, 1],
                mesh_bvhnum: i32 [nmesh, 1],
                mesh_octadr: i32 [nmesh, 1],
                mesh_octnum: i32 [nmesh, 1],
                mesh_normaladr: i32 [nmesh, 1],
                mesh_normalnum: i32 [nmesh, 1],
                mesh_texcoordadr: i32 [nmesh, 1],
                mesh_texcoordnum: i32 [nmesh, 1],
                mesh_graphadr: i32 [nmesh, 1],
                mesh_extrema: i32 [nmesh, 27],
                mesh_vert: f32 [nmeshvert, 3],
                mesh_normal: f32 [nmeshnormal, 3],
                mesh_texcoord: f32 [nmeshtexcoord, 2],
                mesh_face: i32 [nmeshface, 3],
                mesh_facenormal: i32 [nmeshface, 3],
                mesh_facetexcoord: i32 [nmeshface, 3],
                mesh_graph: i32 [nmeshgraph, 1],
                mesh_scale: f64 [nmesh, 3],
                mesh_pos: f64 [nmesh, 3],
                mesh_quat: f64 [nmesh, 4],
                mesh_pathadr: i32 [nmesh, 1],
                mesh_polynum: i32 [nmesh, 1],
                mesh_polyadr: i32 [nmesh, 1],
                mesh_polynormal: f64 [nmeshpoly, 3],
                mesh_polyvertadr: i32 [nmeshpoly, 1],
                mesh_polyvertnum: i32 [nmeshpoly, 1],
                mesh_polyvert: i32 [nmeshpolyvert, 1],
                mesh_polymapadr: i32 [nmeshvert, 1],
                mesh_polymapnum: i32 [nmeshvert, 1],
                mesh_polymap: i32 [nmeshpolymap, 1],
                skin_matid: i32 [nskin, 1],
                skin_group: i32 [nskin, 1],
                skin_rgba: f32 [nskin, 4],
                skin_inflate: f32 [nskin, 1],
                skin_vertadr: i32 [nskin, 1],
                skin_vertnum: i32 [nskin, 1],
                skin_texcoordadr: i32 [nskin, 1],
                skin_faceadr: i32 [nskin, 1],
                skin_facenum: i32 [nskin, 1],
                skin_boneadr: i32 [nskin, 1],
                skin_bonenum: i32 [nskin, 1],
                skin_vert: f32 [nskinvert, 3],
                skin_texcoord: f32 [nskintexvert, 2],
                skin_face: i32 [nskinface, 3],
                skin_bonevertadr: i32 [nskinbone, 1],
                skin_bonevertnum: i32 [nskinbone, 1],
                skin_bonebindpos: f32 [nskinbone, 3],
                skin_bonebindquat: f32 [nskinbone, 4],
                skin_bonebodyid: i32 [nskinbone, 1],
                skin_bonevertid: i32 [nskinbonevert, 1],
                skin_bonevertweight: f32 [nskinbonevert, 1],
                skin_pathadr: i32 [nskin, 1],
                hfield_size: f64 [nhfield, 4],
                hfield_nrow: i32 [nhfield, 1],
                hfield_ncol: i32 [nhfield, 1],
                hfield_adr: i32 [nhfield, 1],
                hfield_data: f32 [nhfielddata, 1],
                hfield_pathadr: i32 [nhfield, 1],
                tex_type: i32 [ntex, 1],
                tex_colorspace: i32 [ntex, 1],
                tex_height: i32 [ntex, 1],
                tex_width: i32 [ntex, 1],
                tex_nchannel: i32 [ntex, 1],
                tex_adr: i64 [ntex, 1],
                tex_data: u8 [ntexdata, 1],
                tex_pathadr: i32 [ntex, 1],
                tex_contenttypeadr: i32 [ntex, 1],
                mat_texid: i32 [nmat, NTEXROLE],
                mat_texuniform: u8 [nmat, 1],
                mat_texrepeat: f32 [nmat, 2],
                mat_emission: f32 [nmat, 1],
                mat_specular: f32 [nmat, 1],
                mat_shininess: f32 [nmat, 1],
                mat_reflectance: f32 [nmat, 1],
                mat_metallic: f32 [nmat, 1],
                mat_roughness: f32 [nmat, 1],
                mat_rgba: f32 [nmat, 4],
                pair_dim: i32 [npair, 1],
                pair_geom1: i32 [npair, 1],
                pair_geom2: i32 [npair, 1],
                pair_signature: i32 [npair, 1],
                pair_solref: f64 [npair, NREF],
                pair_solreffriction: f64 [npair, NREF],
                pair_solimp: f64 [npair, NIMP],
                pair_margin: f64 [npair, 1],
                pair_gap: f64 [npair, 1],
                pair_adhesion: f64 [npair, 1],
                pair_friction: f64 [npair, 5],
                exclude_signature: i32 [nexclude, 1],
                eq_type: i32 [neq, 1],
                eq_obj1id: i32 [neq, 1],
                eq_obj2id: i32 [neq, 1],
                eq_objtype: i32 [neq, 1],
                eq_active0: u8 [neq, 1],
                eq_solref: f64 [neq, NREF],
                eq_solimp: f64 [neq, NIMP],
                eq_data: f64 [neq, NEQDATA],
                tendon_type: i32 [ntendon, 1],
                tendon_adr: i32 [ntendon, 1],
                tendon_num: i32 [ntendon, 1],
                tendon_matid: i32 [ntendon, 1],
                tendon_actuatorid: i32 [ntendon, 1],
                tendon_group: i32 [ntendon, 1],
                tendon_plasticadr: i32 [ntendon, 1],
                tendon_treenum: i32 [ntendon, 1],
                tendon_treeid: i32 [ntendon, 2],
                ten_j_rownnz: i32 [ntendon, 1],
                ten_j_rowadr: i32 [ntendon, 1],
                ten_j_colind: i32 [n_jten, 1],
                tendon_limited: u8 [ntendon, 1],
                tendon_actfrclimited: u8 [ntendon, 1],
                tendon_width: f64 [ntendon, 1],
                tendon_solref_lim: f64 [ntendon, NREF],
                tendon_solimp_lim: f64 [ntendon, NIMP],
                tendon_solref_fri: f64 [ntendon, NREF],
                tendon_solimp_fri: f64 [ntendon, NIMP],
                tendon_range: f64 [ntendon, 2],
                tendon_actfrcrange: f64 [ntendon, 2],
                tendon_margin: f64 [ntendon, 1],
                tendon_stiffness: f64 [ntendon, 1],
                tendon_stiffnesspoly: f64 [ntendon, NPOLY],
                tendon_damping: f64 [ntendon, 1],
                tendon_dampingpoly: f64 [ntendon, NPOLY],
                tendon_armature: f64 [ntendon, 1],
                tendon_frictionloss: f64 [ntendon, 1],
                tendon_lengthspring: f64 [ntendon, 2],
                tendon_springyield: f64 [ntendon, 1],
                tendon_length0: f64 [ntendon, 1],
                tendon_invweight0: f64 [ntendon, 1],
                tendon_user: f64 [ntendon, nuser_tendon],
                tendon_rgba: f32 [ntendon, 4],
                wrap_type: i32 [nwrap, 1],
                wrap_objid: i32 [nwrap, 1],
                wrap_prm: f64 [nwrap, 1],
                actuator_trntype: i32 [nactuator, 1],
                actuator_dyntype: i32 [nactuator, 1],
                actuator_gaintype: i32 [nactuator, 1],
                actuator_biastype: i32 [nactuator, 1],
                actuator_ctrladr: i32 [nactuator, 1],
                actuator_ctrlnum: i32 [nactuator, 1],
                actuator_ctrlspec: i32 [nactuator, 1],
                actuator_outadr: i32 [nactuator, 1],
                actuator_outnum: i32 [nactuator, 1],
                actuator_actadr: i32 [nactuator, 1],
                actuator_actnum: i32 [nactuator, 1],
                actuator_trnid: i32 [nactuator, 2],
                actuator_cranklength: f64 [nactuator, 1],
                actuator_dynprm: f64 [nactuator, NDYN],
                actuator_gainprm: f64 [nactuator, NGAIN],
                actuator_biasprm: f64 [nactuator, NBIAS],
                actuator_actlimited: u8 [nactuator, 1],
                actuator_actrange: f64 [nactuator, 2],
                actuator_actearly: u8 [nactuator, 1],
                actuator_history: i32 [nactuator, 2],
                actuator_historyadr: i32 [nactuator, 1],
                actuator_delay: f64 [nactuator, 1],
                actuator_damping: f64 [nactuator, 1],
                actuator_dampingpoly: f64 [nactuator, NPOLY],
                actuator_armature: f64 [nactuator, 1],
                actuator_group: i32 [nactuator, 1],
                actuator_user: f64 [nactuator, nuser_actuator],
                actuator_plugin: i32 [nactuator, 1],
                actuator_forcelimited: u8 [nactuator, 1],
                actuator_forcerange: f64 [nactuator, 2],
                actuator_ctrllimited: u8 [nu, 1],
                actuator_ctrlrange: f64 [nu, 2],
                actuator_gear: f64 [nout, 6],
                actuator_acc0: f64 [nout, 1],
                actuator_length0: f64 [nout, 1],
                actuator_lengthrange: f64 [nout, 2],
                sensor_type: i32 [nsensor, 1],
                sensor_datatype: i32 [nsensor, 1],
                sensor_needstage: i32 [nsensor, 1],
                sensor_objtype: i32 [nsensor, 1],
                sensor_objid: i32 [nsensor, 1],
                sensor_reftype: i32 [nsensor, 1],
                sensor_refid: i32 [nsensor, 1],
                sensor_intprm: i32 [nsensor, NSENS],
                sensor_dim: i32 [nsensor, 1],
                sensor_adr: i32 [nsensor, 1],
                sensor_cutoff: f64 [nsensor, 1],
                sensor_noise: f64 [nsensor, 1],
                sensor_history: i32 [nsensor, 2],
                sensor_historyadr: i32 [nsensor, 1],
                sensor_delay: f64 [nsensor, 1],
                sensor_interval: f64 [nsensor, 2],
                sensor_user: f64 [nsensor, nuser_sensor],
                sensor_plugin: i32 [nsensor, 1],
                plugin: i32 [nplugin, 1],
                plugin_stateadr: i32 [nplugin, 1],
                plugin_statenum: i32 [nplugin, 1],
                plugin_attr: u8 [npluginattr, 1],
                plugin_attradr: i32 [nplugin, 1],
                numeric_adr: i32 [nnumeric, 1],
                numeric_size: i32 [nnumeric, 1],
                numeric_data: f64 [nnumericdata, 1],
                text_adr: i32 [ntext, 1],
                text_size: i32 [ntext, 1],
                text_data: u8 [ntextdata, 1],
                tuple_adr: i32 [ntuple, 1],
                tuple_size: i32 [ntuple, 1],
                tuple_objtype: i32 [ntupledata, 1],
                tuple_objid: i32 [ntupledata, 1],
                tuple_objprm: f64 [ntupledata, 1],
                key_time: f64 [nkey, 1],
                key_qpos: f64 [nkey, nq],
                key_qvel: f64 [nkey, nv],
                key_act: f64 [nkey, na],
                key_mpos: f64 [nkey, nmocap*3],
                key_mquat: f64 [nkey, nmocap*4],
                key_ctrl: f64 [nkey, nu],
                key_plastic: f64 [nkey, nplastic],
                name_bodyadr: i32 [nbody, 1],
                name_jntadr: i32 [njnt, 1],
                name_geomadr: i32 [ngeom, 1],
                name_siteadr: i32 [nsite, 1],
                name_camadr: i32 [ncam, 1],
                name_lightadr: i32 [nlight, 1],
                name_flexadr: i32 [nflex, 1],
                name_meshadr: i32 [nmesh, 1],
                name_skinadr: i32 [nskin, 1],
                name_hfieldadr: i32 [nhfield, 1],
                name_texadr: i32 [ntex, 1],
                name_matadr: i32 [nmat, 1],
                name_pairadr: i32 [npair, 1],
                name_excludeadr: i32 [nexclude, 1],
                name_eqadr: i32 [neq, 1],
                name_tendonadr: i32 [ntendon, 1],
                name_actuatoradr: i32 [nactuator, 1],
                name_sensoradr: i32 [nsensor, 1],
                name_numericadr: i32 [nnumeric, 1],
                name_textadr: i32 [ntext, 1],
                name_tupleadr: i32 [ntuple, 1],
                name_keyadr: i32 [nkey, 1],
                name_pluginadr: i32 [nplugin, 1],
                names: u8 [nnames, 1],
                names_map: i32 [nnames_map, 1],
                paths: u8 [npaths, 1],
                b_rownnz: i32 [nbody, 1],
                b_rowadr: i32 [nbody, 1],
                b_colind: i32 [n_b, 1],
                m_rownnz: i32 [nv, 1],
                m_rowadr: i32 [nv, 1],
                m_colind: i32 [n_c, 1],
                map_m2m: i32 [n_c, 1],
                d_rownnz: i32 [nv, 1],
                d_rowadr: i32 [nv, 1],
                d_diag: i32 [nv, 1],
                d_colind: i32 [n_d, 1],
                map_m2d: i32 [n_d, 1],
                map_d2m: i32 [n_c, 1],
            }
        }
    };
}

macro_rules! columns {
    ($m:ident; $n:tt * $factor:literal) => { columns!($m; $n) * $factor };
    ($m:ident; nuser_body) => { $m.nuser_body };
    ($m:ident; nuser_jnt) => { $m.nuser_jnt };
    ($m:ident; nuser_geom) => { $m.nuser_geom };
    ($m:ident; nuser_site) => { $m.nuser_site };
    ($m:ident; nuser_cam) => { $m.nuser_cam };
    ($m:ident; nuser_tendon) => { $m.nuser_tendon };
    ($m:ident; nuser_actuator) => { $m.nuser_actuator };
    ($m:ident; nuser_sensor) => { $m.nuser_sensor };
    ($m:ident; nq) => { $m.nq };
    ($m:ident; nv) => { $m.nv };
    ($m:ident; na) => { $m.na };
    ($m:ident; nu) => { $m.nu };
    ($m:ident; nmocap) => { $m.nmocap };
    ($m:ident; nplastic) => { $m.nplastic };
    ($m:ident; $n:expr) => { $n as usize };
}
pub(crate) use columns;

/// Length-range options from mjLROpt (mjmodel.h).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LROpt {
    pub mode: i32,
    pub useexisting: i32,
    pub uselimit: i32,
    pub accel: f64,
    pub maxforce: f64,
    pub timeconst: f64,
    pub timestep: f64,
    pub inttotal: f64,
    pub interval: f64,
    pub tolrange: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub timestep: f64,
    pub impratio: f64,
    pub tolerance: f64,
    pub ls_tolerance: f64,
    pub noslip_tolerance: f64,
    pub ccd_tolerance: f64,
    pub sleep_tolerance: f64,
    pub gravity: [f64; 3],
    pub wind: [f64; 3],
    pub magnetic: [f64; 3],
    pub density: f64,
    pub viscosity: f64,
    pub o_margin: f64,
    pub o_solref: [f64; NREF],
    pub o_solimp: [f64; NIMP],
    pub o_friction: [f64; 5],
    pub integrator: i32,
    pub cone: i32,
    pub jacobian: i32,
    pub solver: i32,
    pub iterations: i32,
    pub ls_iterations: i32,
    pub noslip_iterations: i32,
    pub ccd_iterations: i32,
    pub disableflags: i32,
    pub enableflags: i32,
    pub disableactuator: i32,
    pub sdf_initpoints: i32,
    pub sdf_iterations: i32,
}

impl Default for Opt {
    fn default() -> Self {
        default_option()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statistic {
    pub meaninertia: f64,
    pub meanmass: f64,
    pub meansize: f64,
    pub extent: f64,
    pub center: [f64; 3],
}

impl Default for Statistic {
    fn default() -> Self {
        default_statistic()
    }
}

/// Set default solver parameters (mj_defaultSolRefImp, engine_init.c).
pub fn default_sol_ref_imp(solref: Option<&mut [f64; 2]>, solimp: Option<&mut [f64; 5]>) {
    if let Some(solref) = solref {
        solref[0] = 0.02;
        solref[1] = 1.0;
    }
    if let Some(solimp) = solimp {
        solimp[0] = 0.9;
        solimp[1] = 0.95;
        solimp[2] = 0.001;
        solimp[3] = 0.5;
        solimp[4] = 2.0;
    }
}

/// mj_defaultOption; the serialized padding is explicitly zeroed by the writer.
pub fn default_option() -> Opt {
    Opt {
        timestep: 0.002,
        impratio: 1.0,
        tolerance: 1e-8,
        ls_tolerance: 0.01,
        noslip_tolerance: 1e-6,
        ccd_tolerance: 1e-6,
        sleep_tolerance: 1e-3,
        gravity: [0.0, 0.0, -9.81],
        wind: [0.0, 0.0, 0.0],
        magnetic: [0.0, -0.5, 0.0],
        density: 0.0,
        viscosity: 0.0,
        o_margin: 0.0,
        o_solref: [0.02, 1.0],
        o_solimp: [0.9, 0.95, 0.001, 0.5, 2.0],
        o_friction: [1.0, 1.0, 0.005, 0.0001, 0.0001],
        integrator: INT_EULER,
        cone: CONE_PYRAMIDAL,
        jacobian: JAC_AUTO,
        solver: SOL_NEWTON,
        iterations: 100,
        ls_iterations: 50,
        noslip_iterations: 0,
        ccd_iterations: 35,
        disableflags: 0,
        enableflags: 0,
        disableactuator: 0,
        sdf_initpoints: 40,
        sdf_iterations: 10,
    }
}

pub fn default_statistic() -> Statistic {
    Statistic {
        meaninertia: 1.0,
        meanmass: 1.0,
        meansize: 0.2,
        extent: 2.0,
        center: [0.0; 3],
    }
}

/// mj_defaultLROpt (engine_init.c).
pub fn default_lropt(opt: &mut LROpt) {
    opt.mode = LRMODE_MUSCLE;
    opt.useexisting = 1;
    opt.uselimit = 0;
    opt.accel = 20.0;
    opt.maxforce = 0.0;
    opt.timeconst = 1.0;
    opt.timestep = 0.01;
    opt.inttotal = 10.0;
    opt.interval = 2.0;
    opt.tolrange = 0.05;
}

// mj_defaultVisual initializes the opaque file record, without visualisation code.
fn default_visual() -> Vec<u8> {
    let mut vis = Vec::with_capacity(VISUAL_BYTES);
    // global
    for word in [
        (-1_i32).to_le_bytes(), // cameraid
        (0_i32).to_le_bytes(), // orthographic
        (45.0_f32).to_le_bytes(), // fovy
        (0.068_f32).to_le_bytes(), // ipd
        (90.0_f32).to_le_bytes(), // azimuth
        (-45.0_f32).to_le_bytes(), // elevation
        (1.0_f32).to_le_bytes(), // linewidth
        (0.3_f32).to_le_bytes(), // glow
        (1.0_f32).to_le_bytes(), // realtime
        (640_i32).to_le_bytes(), // offwidth
        (480_i32).to_le_bytes(), // offheight
        (0_i32).to_le_bytes(), // ellipsoidinertia
        (0_i32).to_le_bytes(), // bvactive
    ] {
        vis.extend_from_slice(&word);
    }
    // quality
    for word in [
        (4096_i32).to_le_bytes(), // shadowsize
        (4_i32).to_le_bytes(), // offsamples
        (28_i32).to_le_bytes(), // numslices
        (16_i32).to_le_bytes(), // numstacks
        (4_i32).to_le_bytes(), // numquads
    ] {
        vis.extend_from_slice(&word);
    }
    // headlight
    for word in [
        (0.1_f32).to_le_bytes(), // ambient
        (0.1_f32).to_le_bytes(), // ambient
        (0.1_f32).to_le_bytes(), // ambient
        (0.4_f32).to_le_bytes(), // diffuse
        (0.4_f32).to_le_bytes(), // diffuse
        (0.4_f32).to_le_bytes(), // diffuse
        (0.5_f32).to_le_bytes(), // specular
        (0.5_f32).to_le_bytes(), // specular
        (0.5_f32).to_le_bytes(), // specular
        (2_i32).to_le_bytes(), // active
    ] {
        vis.extend_from_slice(&word);
    }
    // map
    for word in [
        (100.0_f32).to_le_bytes(), // stiffness
        (500.0_f32).to_le_bytes(), // stiffnessrot
        (0.005_f32).to_le_bytes(), // force
        (0.1_f32).to_le_bytes(), // torque
        (0.3_f32).to_le_bytes(), // alpha
        (3.0_f32).to_le_bytes(), // fogstart
        (10.0_f32).to_le_bytes(), // fogend
        (0.01_f32).to_le_bytes(), // znear
        (50.0_f32).to_le_bytes(), // zfar
        (0.3_f32).to_le_bytes(), // haze
        (1.0_f32).to_le_bytes(), // shadowclip
        (0.6_f32).to_le_bytes(), // shadowscale
        (2.0_f32).to_le_bytes(), // actuatortendon
    ] {
        vis.extend_from_slice(&word);
    }
    // scale
    for word in [
        (0.1_f32).to_le_bytes(), // forcewidth
        (0.3_f32).to_le_bytes(), // contactwidth
        (0.1_f32).to_le_bytes(), // contactheight
        (0.2_f32).to_le_bytes(), // connect
        (0.4_f32).to_le_bytes(), // com
        (0.3_f32).to_le_bytes(), // camera
        (0.3_f32).to_le_bytes(), // light
        (0.2_f32).to_le_bytes(), // selectpoint
        (1.0_f32).to_le_bytes(), // jointlength
        (0.1_f32).to_le_bytes(), // jointwidth
        (0.7_f32).to_le_bytes(), // actuatorlength
        (0.2_f32).to_le_bytes(), // actuatorwidth
        (1.0_f32).to_le_bytes(), // framelength
        (0.1_f32).to_le_bytes(), // framewidth
        (0.1_f32).to_le_bytes(), // constraint
        (0.2_f32).to_le_bytes(), // slidercrank
        (10.0_f32).to_le_bytes(), // frustum
    ] {
        vis.extend_from_slice(&word);
    }
    // rgba
    for word in [
        (0.0_f32).to_le_bytes(), // fog
        (0.0_f32).to_le_bytes(), // fog
        (0.0_f32).to_le_bytes(), // fog
        (1.0_f32).to_le_bytes(), // fog
        (1.0_f32).to_le_bytes(), // haze
        (1.0_f32).to_le_bytes(), // haze
        (1.0_f32).to_le_bytes(), // haze
        (1.0_f32).to_le_bytes(), // haze
        (1.0_f32).to_le_bytes(), // force
        (0.5_f32).to_le_bytes(), // force
        (0.5_f32).to_le_bytes(), // force
        (1.0_f32).to_le_bytes(), // force
        (0.8_f32).to_le_bytes(), // inertia
        (0.2_f32).to_le_bytes(), // inertia
        (0.2_f32).to_le_bytes(), // inertia
        (0.6_f32).to_le_bytes(), // inertia
        (0.2_f32).to_le_bytes(), // joint
        (0.6_f32).to_le_bytes(), // joint
        (0.8_f32).to_le_bytes(), // joint
        (1.0_f32).to_le_bytes(), // joint
        (0.2_f32).to_le_bytes(), // actuator
        (0.25_f32).to_le_bytes(), // actuator
        (0.2_f32).to_le_bytes(), // actuator
        (1.0_f32).to_le_bytes(), // actuator
        (0.2_f32).to_le_bytes(), // actuatornegative
        (0.6_f32).to_le_bytes(), // actuatornegative
        (0.9_f32).to_le_bytes(), // actuatornegative
        (1.0_f32).to_le_bytes(), // actuatornegative
        (0.9_f32).to_le_bytes(), // actuatorpositive
        (0.4_f32).to_le_bytes(), // actuatorpositive
        (0.2_f32).to_le_bytes(), // actuatorpositive
        (1.0_f32).to_le_bytes(), // actuatorpositive
        (0.9_f32).to_le_bytes(), // com
        (0.9_f32).to_le_bytes(), // com
        (0.9_f32).to_le_bytes(), // com
        (1.0_f32).to_le_bytes(), // com
        (0.6_f32).to_le_bytes(), // camera
        (0.9_f32).to_le_bytes(), // camera
        (0.6_f32).to_le_bytes(), // camera
        (1.0_f32).to_le_bytes(), // camera
        (0.6_f32).to_le_bytes(), // light
        (0.6_f32).to_le_bytes(), // light
        (0.9_f32).to_le_bytes(), // light
        (1.0_f32).to_le_bytes(), // light
        (0.9_f32).to_le_bytes(), // selectpoint
        (0.9_f32).to_le_bytes(), // selectpoint
        (0.1_f32).to_le_bytes(), // selectpoint
        (1.0_f32).to_le_bytes(), // selectpoint
        (0.2_f32).to_le_bytes(), // connect
        (0.2_f32).to_le_bytes(), // connect
        (0.8_f32).to_le_bytes(), // connect
        (1.0_f32).to_le_bytes(), // connect
        (0.9_f32).to_le_bytes(), // contactpoint
        (0.6_f32).to_le_bytes(), // contactpoint
        (0.2_f32).to_le_bytes(), // contactpoint
        (1.0_f32).to_le_bytes(), // contactpoint
        (0.7_f32).to_le_bytes(), // contactforce
        (0.9_f32).to_le_bytes(), // contactforce
        (0.9_f32).to_le_bytes(), // contactforce
        (1.0_f32).to_le_bytes(), // contactforce
        (0.9_f32).to_le_bytes(), // contactfriction
        (0.8_f32).to_le_bytes(), // contactfriction
        (0.4_f32).to_le_bytes(), // contactfriction
        (1.0_f32).to_le_bytes(), // contactfriction
        (0.9_f32).to_le_bytes(), // contacttorque
        (0.7_f32).to_le_bytes(), // contacttorque
        (0.9_f32).to_le_bytes(), // contacttorque
        (1.0_f32).to_le_bytes(), // contacttorque
        (0.5_f32).to_le_bytes(), // contactgap
        (0.8_f32).to_le_bytes(), // contactgap
        (0.9_f32).to_le_bytes(), // contactgap
        (1.0_f32).to_le_bytes(), // contactgap
        (1.0_f32).to_le_bytes(), // rangefinder
        (1.0_f32).to_le_bytes(), // rangefinder
        (0.1_f32).to_le_bytes(), // rangefinder
        (1.0_f32).to_le_bytes(), // rangefinder
        (0.9_f32).to_le_bytes(), // constraint
        (0.0_f32).to_le_bytes(), // constraint
        (0.0_f32).to_le_bytes(), // constraint
        (1.0_f32).to_le_bytes(), // constraint
        (0.5_f32).to_le_bytes(), // slidercrank
        (0.3_f32).to_le_bytes(), // slidercrank
        (0.8_f32).to_le_bytes(), // slidercrank
        (1.0_f32).to_le_bytes(), // slidercrank
        (0.9_f32).to_le_bytes(), // crankbroken
        (0.0_f32).to_le_bytes(), // crankbroken
        (0.0_f32).to_le_bytes(), // crankbroken
        (1.0_f32).to_le_bytes(), // crankbroken
        (1.0_f32).to_le_bytes(), // frustum
        (1.0_f32).to_le_bytes(), // frustum
        (0.0_f32).to_le_bytes(), // frustum
        (0.2_f32).to_le_bytes(), // frustum
        (0.0_f32).to_le_bytes(), // bv
        (1.0_f32).to_le_bytes(), // bv
        (0.0_f32).to_le_bytes(), // bv
        (0.5_f32).to_le_bytes(), // bv
        (1.0_f32).to_le_bytes(), // bvactive
        (0.0_f32).to_le_bytes(), // bvactive
        (0.0_f32).to_le_bytes(), // bvactive
        (0.5_f32).to_le_bytes(), // bvactive
    ] {
        vis.extend_from_slice(&word);
    }
    vis
}

macro_rules! define_model {
    (() sizes { $($size:ident,)* } arrays { $($name:ident: $ty:ident [$nr:ident, $($nc:tt)+],)* }) => {
        pub const MODEL_SIZES: &[&str] = &[$(stringify!($size),)*];
        pub const MODEL_POINTERS: usize = [$(stringify!($name),)*].len();
        /// The first 84 MJMODEL_SIZES are the arguments to mj_makeModel.
        pub const CONSTRUCTION_SIZES: usize = 84;

        #[derive(Clone, Debug, PartialEq)]
        pub struct Model {
            $(pub $size: usize,)*
            pub opt: Opt,
            pub vis: Vec<u8>,
            pub stat: Statistic,
            pub flg_gravcomp: u8,
            pub flg_surfacevel: u8,
            pub flg_adhesion: u8,
            $(pub $name: Vec<$ty>,)*
            /// The compilation signature; not in the model file, so 0 when loaded.
            pub signature: u64,
            pub sdf_plugins: Vec<crate::sdf::SdfPlugin>,
        }

        impl Model {
            pub(crate) fn set_sizes(&mut self, sizes: &[i64]) {
                let mut values = sizes.iter();
                $(self.$size = *values.next().unwrap() as usize;)*
            }

            pub(crate) fn sizes(&self) -> [i64; MODEL_SIZES.len()] {
                [$(self.$size as i64,)*]
            }
        }

        /// Allocate from the first CONSTRUCTION_SIZES entries in MJMODEL_SIZES order.
        /// Extra entries are ignored, as in the C loader's call to mj_makeModel.
        /// Post-construction sizes start at zero, except nnames_map and nbuffer.
        pub fn make_model(sizes: &[i64]) -> Result<Model, String> {
            if sizes.len() < CONSTRUCTION_SIZES {
                return Err("Invalid sizes, unable to load model".into());
            }
            let mut construction = [0_i64; MODEL_SIZES.len()];
            construction[..CONSTRUCTION_SIZES].copy_from_slice(&sizes[..CONSTRUCTION_SIZES]);
            for (name, &value) in MODEL_SIZES.iter().zip(&construction) {
                if value < 0 {
                    return Err(format!("Invalid model: {name} is negative ({value})."));
                }
                if value >= i32::MAX as i64 && *name != "ntexdata" && *name != "ntextdata" {
                    return Err(format!(
                        "Invalid model: {name} is too large. Expected < {}. Got {value}.",
                        i32::MAX,
                    ));
                }
            }
            let mut m = Model {
                $($size: 0,)*
                opt: default_option(),
                vis: default_visual(),
                stat: default_statistic(),
                flg_gravcomp: 0,
                flg_surfacevel: 0,
                flg_adhesion: 0,
                $($name: Vec::new(),)*
                signature: 0,
                sdf_plugins: Vec::new(),
            };
            m.set_sizes(&construction);
            if m.nbody == 0 {
                return Err("Invalid model: nbody == 0".into());
            }
            let nnames_map = m.nbody + m.njnt + m.ngeom + m.nsite + m.ncam + m.nlight
                + m.nflex + m.nmesh + m.nskin + m.nhfield + m.ntex + m.nmat + m.npair
                + m.nexclude + m.neq + m.ntendon + m.nactuator + m.nsensor
                + m.nnumeric + m.ntext + m.ntuple + m.nkey + m.nplugin;
            // mjLOAD_MULTIPLE is 2 (engine_io.h).
            if nnames_map >= i32::MAX as usize / 2 {
                return Err("Invalid model: size of nnames_map is larger than INT_MAX".into());
            }
            m.nnames_map = 2 * nnames_map;

            // BUFFER_ALIGN = 64. The first array reserves the maximum possible
            // alignment padding (63); each later array includes SKIP(offset).
            let mut offset = 0_usize;
            m.nbuffer = 63;
            $(safe_add_to_buffer_size(
                &mut offset, &mut m.nbuffer, std::mem::size_of::<$ty>(),
                m.$nr, columns!(m; $($nc)+), stringify!($name),
            )?;)*
            $(m.$name = zeroed(m.$nr * columns!(m; $($nc)+))?;)*
            Ok(m)
        }
    };
}
model_fields!(define_model());

fn safe_add_to_buffer_size(
    offset: &mut usize, nbuffer: &mut usize, type_size: usize,
    nr: usize, nc: usize, name: &str,
) -> Result<(), String> {
    let overflow = || format!("Invalid model: {name} too large.");
    let skip = (64 - (*offset % 64)) % 64;
    let to_add = nc.checked_mul(nr)
        .and_then(|size| size.checked_mul(type_size))
        .and_then(|size| size.checked_add(skip))
        .ok_or_else(overflow)?;
    *nbuffer = nbuffer.checked_add(to_add)
        .filter(|&size| size <= i64::MAX as usize).ok_or_else(overflow)?;
    *offset = offset.checked_add(to_add)
        .filter(|&size| size <= isize::MAX as usize).ok_or_else(overflow)?;
    Ok(())
}

fn zeroed<T: Default + Clone>(len: usize) -> Result<Vec<T>, String> {
    let mut values = Vec::new();
    values.try_reserve_exact(len).map_err(|_| "could not allocate the model buffer".to_string())?;
    values.resize(len, T::default());
    Ok(values)
}

pub fn enabled(m: &Model, flag: i32) -> bool {
    m.opt.enableflags & flag != 0
}

pub fn disabled(m: &Model, flag: i32) -> bool {
    m.opt.disableflags & flag != 0
}

/// Refuse a model that needs a feature this port does not translate yet,
/// so a model that loads never reaches an `unported!` call.
pub fn check_supported(m: &Model) -> Result<(), String> {
    let refuse = |what: &str| Err(format!("this model uses {what}, which is not supported yet"));
    if m.nflex != 0 {
        return refuse("flexes");
    }
    if m.opt.integrator == INT_DISCRETE {
        return refuse("the discrete integrator");
    }
    if enabled(m, ENBL_IPC) {
        return refuse("IPC contacts");
    }
    if disabled(m, DSBL_NATIVECCD) {
        return refuse("the external convex collision library");
    }
    if m.sdf_plugins.len() != m.nplugin {
        return refuse("plugins without attached SDF fields");
    }
    for (geom, &instance) in m.geom_plugin.iter().enumerate() {
        if instance >= 0
            && (m.geom_type[geom] != GEOM_SDF || instance as usize >= m.sdf_plugins.len())
        {
            return refuse("non-SDF or unattached geom plugins");
        }
    }
    if m.nhistory != 0 {
        return refuse("actuator or sensor delays");
    }
    for (geom, &geom_type) in m.geom_type.iter().enumerate() {
        if geom_type == GEOM_SDF && m.geom_plugin[geom] < 0 {
            let mesh = m.geom_dataid[geom];
            if mesh < 0
                || mesh as usize >= m.mesh_octadr.len()
                || m.mesh_octadr[mesh as usize] == -1
            {
                return refuse("SDF geoms without a plugin or octree");
            }
        }
    }
    if m.sensor_type.iter().any(|&t| t == SENS_RANGEFINDER) {
        return refuse("rangefinder sensors");
    }
    Ok(())
}
