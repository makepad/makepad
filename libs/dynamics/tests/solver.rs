// Invoked inside solver so its C file-local projection helpers stay private.
#[macro_export]
macro_rules! solver_tests {
    () => {
        #[test]
        fn friction_cone_and_ellipsoid_projection() {
            use super::{project_cone, project_ellipsoid};
            use crate::types::{CNSTR_CONTACT_ELLIPTIC, CNSTR_CONTACT_PYRAMIDAL};

            // C: s = 6^2/2^2 + 16^2/4^2 = 9+16 = 25; normal^2 = 100,
            // so forced boundary projection scales by sqrt(100/25) = 2.
            let mu = [2.0, 4.0];
            let mut friction = [6.0, 16.0];
            project_ellipsoid(&mut friction, 10.0, &mu, 3, 1);
            assert_eq!(friction, [6.0, 16.0], "feasible interior stays put");
            project_ellipsoid(&mut friction, 10.0, &mu, 3, 0);
            assert_eq!(friction, [12.0, 32.0], "forced projection reaches boundary");

            // C: s = 12^2/2^2 + 32^2/4^2 = 36+64 = 100;
            // normal^2 = 25, hence scale = sqrt(25/100) = 0.5.
            let mut force = [5.0, 12.0, 32.0];
            project_cone(&mut force, Some(&mu), 3, CNSTR_CONTACT_ELLIPTIC);
            assert_eq!(force, [5.0, 6.0, 16.0], "outside force scales down");
            force[0] = -1.0;
            project_cone(&mut force, Some(&mu), 3, CNSTR_CONTACT_ELLIPTIC);
            assert_eq!(force, [0.0; 3], "negative normal clears elliptic block");
            force = [-1.0, 6.0, 16.0];
            project_cone(&mut force, None, 1, CNSTR_CONTACT_PYRAMIDAL);
            assert_eq!(force, [0.0, 6.0, 16.0], "pyramid clamps only its scalar edge");
        }
        #[test]
        fn friction_huber_cost_and_difference() {
            use super::{friction_cost, friction_cost_dif};

            // Hand trace of C with f=4, R=0.5, Rf=2, D=2:
            // quadratic x=1: .5*2*1*1=1; boundary x=+-2: 4;
            // linear x=+-3: 4*(-1+3)=8. Each boundary uses its linear branch.
            assert_eq!(friction_cost(1.0, 4.0, 2.0, 2.0), 1.0, "quadratic cost");
            for x in [-2.0, 2.0] {
                assert_eq!(friction_cost(x, 4.0, 2.0, 2.0), 4.0, "boundary cost");
            }
            for x in [-3.0, 3.0] {
                assert_eq!(friction_cost(x, 4.0, 2.0, 2.0), 8.0, "linear cost");
            }
            // Quadratic delta: .5*2*(1-.5)*(1+.5)=.75.
            assert_eq!(friction_cost_dif(0.5, 1.0, 4.0, 2.0, 2.0), 0.75, "quadratic delta");
            assert_eq!(friction_cost_dif(-3.0, -4.0, 4.0, 2.0, 2.0), 4.0, "negative linear delta");
            assert_eq!(friction_cost_dif(3.0, 4.0, 4.0, 2.0, 2.0), 4.0, "positive linear delta");
            assert_eq!(friction_cost_dif(1.0, -3.0, 4.0, 2.0, 2.0), 7.0, "zone crossing delta");
            assert_eq!(friction_cost_dif(-3.0, 1.0, 4.0, 2.0, 2.0), -7.0, "reverse crossing delta");
        }
    };
}
