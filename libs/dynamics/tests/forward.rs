use makepad_dynamics::forward::{forward, step};
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::{MODEL_SIZES, make_model};
use makepad_dynamics::support::{get_state, state_size};
use makepad_dynamics::types::{INT_EULER, INT_IMPLICIT, INT_IMPLICITFAST, INT_RK4, STATE_INTEGRATION};

#[test]
fn step_empty_world() {
    let mut sizes = vec![0; MODEL_SIZES.len()];
    sizes[MODEL_SIZES.iter().position(|&name| name == "nbody").unwrap()] = 1;
    let mut m = make_model(&sizes).unwrap();
    m.body_treeid[0] = -1;
    m.body_mocapid[0] = -1;
    for integrator in [INT_EULER, INT_RK4, INT_IMPLICIT, INT_IMPLICITFAST] {
        m.opt.integrator = integrator;
        let mut d = make_data(&m);
        // Establish derived world quantities before comparing the step's state.
        forward(&m, &mut d);
        let mut before = vec![0.0; state_size(&m, STATE_INTEGRATION)];
        get_state(&m, &d, &mut before, STATE_INTEGRATION);
        let xpos = d.xpos.clone();
        let xquat = d.xquat.clone();
        let time = d.time;
        step(&m, &mut d);
        assert_eq!(d.time, time + m.opt.timestep, "integrator {integrator}: time");
        let mut after = vec![0.0; before.len()];
        get_state(&m, &d, &mut after, STATE_INTEGRATION);
        assert_eq!(&after[1..], &before[1..], "integrator {integrator}: integration state except time");
        assert_eq!(d.xpos, xpos, "integrator {integrator}: world position");
        assert_eq!(d.xquat, xquat, "integrator {integrator}: world orientation");
        assert!(d.qacc.is_empty(), "integrator {integrator}: no accelerations");
        assert_eq!(d.ncon, 0, "integrator {integrator}: no contacts");
        assert_eq!(d.nefc, 0, "integrator {integrator}: no constraints");
        assert!(d.warning.iter().all(|w| w.number == 0), "integrator {integrator}: no warnings");
    }
}
