use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real, SimdBool, SimdReal, Vector, Vector, DIM, SIMD_WIDTH};
use crate::query::SimdRay;
use crate::utils::{self, PoseOps};
use num::{One, Zero};
use simba::simd::{SimdPartialOrd, SimdValue};

/// Four Aabb represented as a single SoA Aabb with SIMD components.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize),
)]
pub struct SimdAabb {
    /// The min coordinates of the Aabbs.
    pub mins: Vector<SimdReal>,
    /// The max coordinates the Aabbs.
    pub maxs: Vector<SimdReal>,
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for SimdAabb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mins: Vector<[Real; SIMD_WIDTH]> =
            Vector::from(self.mins.map(|e| array![|ii| e.extract(ii); SIMD_WIDTH]));
        let maxs: Vector<[Real; SIMD_WIDTH]> =
            Vector::from(self.maxs.map(|e| array![|ii| e.extract(ii); SIMD_WIDTH]));

        let mut simd_aabb = serializer.serialize_struct("SimdAabb", 2)?;
        simd_aabb.serialize_field("mins", &mins)?;
        simd_aabb.serialize_field("maxs", &maxs)?;
        simd_aabb.end()
    }
}

#[cfg(feature = "serde-serialize")]
impl<'de> serde::Deserialize<'de> for SimdAabb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor {}

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Mins,
            Maxs,
        }

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = SimdAabb;
            fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                write!(
                    formatter,
                    "two arrays containing at least {} floats",
                    SIMD_WIDTH * DIM * 2
                )
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut mins: Option<Vector<[Real; SIMD_WIDTH]>> = None;
                let mut maxs: Option<Vector<[Real; SIMD_WIDTH]>> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Mins => {
                            if mins.is_some() {
                                return Err(serde::de::Error::duplicate_field("mins"));
                            }
                            mins = Some(map.next_value()?);
                        }
                        Field::Maxs => {
                            if maxs.is_some() {
                                return Err(serde::de::Error::duplicate_field("maxs"));
                            }
                            maxs = Some(map.next_value()?);
                        }
                    }
                }

                let mins = mins.ok_or_else(|| serde::de::Error::missing_field("mins"))?;
                let maxs = maxs.ok_or_else(|| serde::de::Error::missing_field("maxs"))?;
                let mins = mins.map(SimdReal::from);
                let maxs = maxs.map(SimdReal::from);
                Ok(SimdAabb { mins, maxs })
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mins: Vector<[Real; SIMD_WIDTH]> = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let maxs: Vector<[Real; SIMD_WIDTH]> = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let mins = mins.map(SimdReal::from);
                let maxs = maxs.map(SimdReal::from);
                Ok(SimdAabb { mins, maxs })
            }
        }

        deserializer.deserialize_struct("SimdAabb", &["mins", "maxs"], Visitor {})
    }
}

impl SimdAabb {
    /// An invalid Aabb.
    pub fn new_invalid() -> Self {
        Self::splat(Aabb::new_invalid())
    }

    /// Builds an SIMD aabb composed of four identical aabbs.
    pub fn splat(aabb: Aabb) -> Self {
        Self {
            mins: Vector::splat(aabb.mins),
            maxs: Vector::splat(aabb.maxs),
        }
    }

    /// The center of all the Aabbs represented by `self`.
    pub fn center(&self) -> Vector<SimdReal> {
        Vector::from((self.mins + self.maxs) * SimdReal::splat(0.5))
    }

    /// The half-extents of all the Aabbs represented by `self`.
    pub fn half_extents(&self) -> Vector<SimdReal> {
        (self.maxs - self.mins) * SimdReal::splat(0.5)
    }

    /// The radius of all the Aabbs represented by `self`.
    pub fn radius(&self) -> SimdReal {
        (self.maxs - self.mins).length()
    }

    /// Return the Aabb of the `self` transformed by the given isometry.
    pub fn transform_by(&self, transform: &Pose<SimdReal>) -> Self {
        let ls_center = self.center();
        let center = transform * ls_center;
        let ws_half_extents = transform.absolute_transform_vector(&self.half_extents());
        Self {
            mins: center + (-ws_half_extents),
            maxs: center + ws_half_extents,
        }
    }

    /// Returns a scaled version of this Aabb.
    #[inline]
    pub fn scaled(self, scale: Vector<SimdReal>) -> Self {
        let a = self.mins * (scale);
        let b = self.maxs * (scale);
        Self {
            mins: a.min(b).into(),
            maxs: a.max(b).into(),
        }
    }

    /// Enlarges this bounding volume by the given margin.
    pub fn loosen(&mut self, margin: SimdReal) {
        self.mins -= Vector::splat(margin);
        self.maxs += Vector::splat(margin);
    }

    /// Dilate all the Aabbs represented by `self` by their extents multiplied
    /// by the given scale `factor`.
    pub fn dilate_by_factor(&mut self, factor: SimdReal) {
        // If some of the Aabbs on this SimdAabb are invalid,
        // don't, dilate them.
        let is_valid = self.mins.x.simd_le(self.maxs.x);
        let factor = factor.select(is_valid, SimdReal::zero());

        // NOTE: we multiply each by factor instead of doing
        // (maxs - mins) * factor. That's to avoid overflows (and
        // therefore NaNs if this SimdAabb contains some invalid
        // Aabbs initialised with Real::MAX
        let dilation = self.maxs * factor - self.mins * factor;
        self.mins -= dilation;
        self.maxs += dilation;
    }

    /// Replace the `i-th` Aabb of this SIMD AAAB by the given value.
    pub fn replace(&mut self, i: usize, aabb: Aabb) {
        self.mins.replace(i, aabb.mins);
        self.maxs.replace(i, aabb.maxs);
    }

    /// Casts a ray on all the Aabbs represented by `self`.
    pub fn cast_local_ray(
        &self,
        ray: &SimdRay,
        max_time_of_impact: SimdReal,
    ) -> (SimdBool, SimdReal) {
        let zero = SimdReal::zero();
        let one = SimdReal::one();
        let infinity = SimdReal::splat(Real::MAX);

        let mut hit = SimdBool::splat(true);
        let mut tmin = SimdReal::zero();
        let mut tmax = max_time_of_impact;

        // TODO: could this be optimized more considering we really just need a boolean answer?
        for i in 0usize..DIM {
            let is_not_zero = ray.dir[i].simd_ne(zero);
            let is_zero_test =
                ray.origin[i].simd_ge(self.mins[i]) & ray.origin[i].simd_le(self.maxs[i]);
            let is_not_zero_test = {
                let denom = one / ray.dir[i];
                let mut inter_with_near_plane =
                    ((self.mins[i] - ray.origin[i]) * denom).select(is_not_zero, -infinity);
                let mut inter_with_far_plane =
                    ((self.maxs[i] - ray.origin[i]) * denom).select(is_not_zero, infinity);

                let gt = inter_with_near_plane.simd_gt(inter_with_far_plane);
                utils::simd_swap(gt, &mut inter_with_near_plane, &mut inter_with_far_plane);

                tmin = tmin.simd_max(inter_with_near_plane);
                tmax = tmax.simd_min(inter_with_far_plane);

                tmin.simd_le(tmax)
            };

            hit = hit & is_not_zero_test.select(is_not_zero, is_zero_test);
        }

        (hit, tmin)
    }

    /// Computes the distances between a point and all the Aabbs represented by `self`.
    pub fn distance_to_local_point(&self, point: Vector<SimdReal>) -> SimdReal {
        let mins_point = self.mins - point;
        let point_maxs = point - self.maxs;
        let shift = mins_point.max(point_maxs).max(Vector::ZERO);
        shift.length()
    }

    /// Computes the distances between the origin and all the Aabbs represented by `self`.
    pub fn distance_to_origin(&self) -> SimdReal {
        self.mins.max(-self.maxs).max(Vector::ZERO).length()
    }

    /// Check which Aabb represented by `self` contains the given `point`.
    pub fn contains_local_point(&self, point: Vector<SimdReal>) -> SimdBool {
        #[cfg(feature = "dim2")]
        return self.mins.x.simd_le(point.x)
            & self.mins.y.simd_le(point.y)
            & self.maxs.x.simd_ge(point.x)
            & self.maxs.y.simd_ge(point.y);

        #[cfg(feature = "dim3")]
        return self.mins.x.simd_le(point.x)
            & self.mins.y.simd_le(point.y)
            & self.mins.z.simd_le(point.z)
            & self.maxs.x.simd_ge(point.x)
            & self.maxs.y.simd_ge(point.y)
            & self.maxs.z.simd_ge(point.z);
    }

    /// Lanewise check which Aabb represented by `self` contains the given set of `other` aabbs.
    /// The check is performed lane-wise.
    #[cfg(feature = "dim2")]
    pub fn contains(&self, other: &SimdAabb) -> SimdBool {
        self.mins.x.simd_le(other.mins.x)
            & self.mins.y.simd_le(other.mins.y)
            & self.maxs.x.simd_ge(other.maxs.x)
            & self.maxs.y.simd_ge(other.maxs.y)
    }

    /// Lanewise check which Aabb represented by `self` contains the given set of `other` aabbs.
    /// The check is performed lane-wise.
    #[cfg(feature = "dim3")]
    pub fn contains(&self, other: &SimdAabb) -> SimdBool {
        self.mins.x.simd_le(other.mins.x)
            & self.mins.y.simd_le(other.mins.y)
            & self.mins.z.simd_le(other.mins.z)
            & self.maxs.x.simd_ge(other.maxs.x)
            & self.maxs.y.simd_ge(other.maxs.y)
            & self.maxs.z.simd_ge(other.maxs.z)
    }

    /// Lanewise check which Aabb represented by `self` intersects the given set of `other` aabbs.
    /// The check is performed lane-wise.
    #[cfg(feature = "dim2")]
    pub fn intersects(&self, other: &SimdAabb) -> SimdBool {
        self.mins.x.simd_le(other.maxs.x)
            & other.mins.x.simd_le(self.maxs.x)
            & self.mins.y.simd_le(other.maxs.y)
            & other.mins.y.simd_le(self.maxs.y)
    }

    /// Check which Aabb represented by `self` contains the given set of `other` aabbs.
    /// The check is performed lane-wise.
    #[cfg(feature = "dim3")]
    pub fn intersects(&self, other: &SimdAabb) -> SimdBool {
        self.mins.x.simd_le(other.maxs.x)
            & other.mins.x.simd_le(self.maxs.x)
            & self.mins.y.simd_le(other.maxs.y)
            & other.mins.y.simd_le(self.maxs.y)
            & self.mins.z.simd_le(other.maxs.z)
            & other.mins.z.simd_le(self.maxs.z)
    }

    /// Checks intersections between all the lanes combination between `self` and `other`.
    ///
    /// The result is an array such that `result[i].extract(j)` contains the intersection
    /// result between `self.extract(i)` and `other.extract(j)`.
    pub fn intersects_permutations(&self, other: &SimdAabb) -> [SimdBool; SIMD_WIDTH] {
        let mut result = [SimdBool::splat(false); SIMD_WIDTH];
        for (ii, result) in result.iter_mut().enumerate() {
            // TODO: use SIMD-accelerated shuffling?
            let extracted = SimdAabb::splat(self.extract(ii));
            *result = extracted.intersects(other);
        }

        result
    }

    /// Merge all the Aabb represented by `self` into a single one.
    pub fn to_merged_aabb(&self) -> Aabb {
        Aabb::new(
            self.mins.map(|e| e.simd_horizontal_min()).into(),
            self.maxs.map(|e| e.simd_horizontal_max()).into(),
        )
    }

    /// Extracts the Aabb stored in the given SIMD lane of the SIMD Aabb:
    pub fn extract(&self, lane: usize) -> Aabb {
        Aabb::new(self.mins.extract(lane), self.maxs.extract(lane))
    }
}

impl From<[Aabb; SIMD_WIDTH]> for SimdAabb {
    fn from(aabbs: [Aabb; SIMD_WIDTH]) -> Self {
        let mins = array![|ii| aabbs[ii].mins; SIMD_WIDTH];
        let maxs = array![|ii| aabbs[ii].maxs; SIMD_WIDTH];

        SimdAabb {
            mins: Vector::from(mins),
            maxs: Vector::from(maxs),
        }
    }
}
