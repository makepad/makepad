// Body-local items and their derived impls must be collected before method resolution,
// including when the methods are used from a nested closure or before the declaration.
#[test]
fn local_struct_derives() {
    let empty = JobLine::default();
    assert_eq!(format!("{:?}", empty), "JobLine { prompt: \"\", seed: 0 }");

    #[derive(Debug, Clone, PartialEq, Default)]
    struct JobLine {
        prompt: String,
        seed: u64,
    }

    let job = JobLine { prompt: String::from("image"), seed: 7 };
    assert!(job.clone().eq(&job));
    assert!(!job.eq(&empty));
    let outer = || {
        let inner = || {
            assert!(JobLine::default().eq(&empty));
            assert!(job.clone().eq(&job));
            format!("{:?}", job)
        };
        inner()
    };
    assert_eq!(outer(), "JobLine { prompt: \"image\", seed: 7 }");
}

#[test]
fn local_enum_derives() {
    #[derive(Debug, Clone, PartialEq, Default)]
    enum State {
        #[default]
        Idle,
        Ready(u32),
    }

    let idle = State::default();
    let ready = State::Ready(9);
    assert!(idle.eq(&State::Idle));
    assert!(ready.clone().eq(&ready));
    assert!(!ready.eq(&idle));
    assert_eq!(format!("{:?}", idle), "Idle");
    let outer = || {
        let inner = || {
            assert!(State::default().eq(&idle));
            assert!(ready.clone().eq(&ready));
            format!("{:?}", ready)
        };
        inner()
    };
    assert_eq!(outer(), "Ready(9)");
}

#[test]
fn local_derives_in_macro_and_closure() {
    macro_rules! declare {
        () => {
            #[derive(Debug, Clone, PartialEq, Default)]
            struct Local(u32);
        };
    }

    declare!();
    let value = Local(5);
    let nested = || {
        // This has the same name, but its impls belong to this block's type.
        #[derive(Debug, Clone, PartialEq, Default)]
        struct Local {
            flag: bool,
        }

        let local = Local::default();
        assert!(local.clone().eq(&Local { flag: false }));
        assert_eq!(format!("{:?}", local), "Local { flag: false }");
        assert!(value.clone().eq(&value));
    };
    nested();
    assert!(Local::default().eq(&Local(0)));
    assert_eq!(format!("{:?}", value), "Local(5)");
}

#[test]
fn local_derives_respect_cfg() {
    // Must be stripped before derive expansion: this enum has no default variant.
    #[cfg(any())]
    #[derive(Debug, Clone, PartialEq, Default)]
    enum Disabled {
        NoDefault,
    }

    #[cfg_attr(all(), derive(Debug, Clone, PartialEq, Default))]
    struct Active(u32);

    let value = Active::default();
    assert!(value.clone().eq(&Active(0)));
    assert_eq!(format!("{:?}", value), "Active(0)");
}
