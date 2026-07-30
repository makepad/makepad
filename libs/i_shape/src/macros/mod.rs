#[macro_export]
macro_rules! int_path {
    ( $( [$x:expr, $y:expr] ),* $(,)? ) => {
        {
            let mut path: $crate::base::data::Path<_> = ::core::default::Default::default();
            $(
                path.push($crate::int::IntPoint::new($x, $y));
            )*
            path
        }
    };
}

#[macro_export]
macro_rules! int_shape {
    ( $( [ $( [$x:expr, $y:expr] ),* $(,)? ] ),* $(,)? ) => {
        {
            let mut shape: $crate::base::data::Shape<_> = ::core::default::Default::default();
            $(
                let mut contour: $crate::base::data::Contour<_> = ::core::default::Default::default();
                $(
                    contour.push($crate::int::IntPoint::new($x, $y));
                )*
                shape.push(contour);
            )*
            shape
        }
    };
}

#[macro_export]
macro_rules! int_shapes {
    ( $( [ $( [ $( [$x:expr, $y:expr] ),* $(,)? ] ),* $(,)? ] ),* $(,)? ) => {
        {
            let mut shapes: $crate::base::data::Shapes<_> = ::core::default::Default::default();
            $(
                let mut shape: $crate::base::data::Shape<_> = ::core::default::Default::default();
                $(
                    let mut contour: $crate::base::data::Contour<_> = ::core::default::Default::default();
                    $(
                        contour.push($crate::int::IntPoint::new($x, $y));
                    )*
                    shape.push(contour);
                )*
                shapes.push(shape);
            )*
            shapes
        }
    };
}

#[macro_export]
macro_rules! path {
    ( $( $point:expr ),* $(,)? ) => {
        {
            let mut path: $crate::base::data::Path<_> = ::core::default::Default::default();
            $(
                path.push($point);
            )*
            path
        }
    };
}

#[macro_export]
macro_rules! paths {
    ( $( [ $( $point:expr ),* $(,)? ] ),* $(,)? ) => {
        {
            let mut paths: $crate::base::data::Paths<_> = ::core::default::Default::default();
            $(
                let mut path: $crate::base::data::Path<_> = ::core::default::Default::default();
                $(
                    path.push($point);
                )*
                paths.push(path);
            )*
            paths
        }
    };
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use i_float::int::point::IntPoint;

    #[test]
    fn int_path_macro_builds_vectors() {
        let path = int_path![[0, 1], [2, 3], [4, 5]];
        assert_eq!(
            path,
            vec![IntPoint::new(0, 1), IntPoint::new(2, 3), IntPoint::new(4, 5),]
        );
    }

    #[test]
    fn int_shape_macro_builds_nested_vectors() {
        let shape = int_shape![[[0, 0], [1, 0], [1, 1]], [[2, 2], [3, 2], [3, 3], [2, 3]],];

        assert_eq!(
            shape,
            vec![
                vec![IntPoint::new(0, 0), IntPoint::new(1, 0), IntPoint::new(1, 1),],
                vec![
                    IntPoint::new(2, 2),
                    IntPoint::new(3, 2),
                    IntPoint::new(3, 3),
                    IntPoint::new(2, 3),
                ],
            ]
        );
    }

    #[test]
    fn int_shapes_macro_builds_multiple_shapes() {
        let shapes = int_shapes![
            [[[0, 0], [1, 0], [1, 1]], [[2, 0], [3, 0], [3, 1]],],
            [[[10, 10], [11, 10], [11, 11], [10, 11]],],
        ];

        assert_eq!(
            shapes,
            vec![
                vec![
                    vec![IntPoint::new(0, 0), IntPoint::new(1, 0), IntPoint::new(1, 1),],
                    vec![IntPoint::new(2, 0), IntPoint::new(3, 0), IntPoint::new(3, 1),],
                ],
                vec![vec![
                    IntPoint::new(10, 10),
                    IntPoint::new(11, 10),
                    IntPoint::new(11, 11),
                    IntPoint::new(10, 11),
                ],],
            ]
        );
    }
}
