use crate::*;
use fe::EccFieldElement;
use rand_core::{CryptoRng, RngCore};
use std::fmt;

/// Elliptic curve type enum
///
/// Enumerates the curves supported by this library, currently K256 (aka
/// secp256k1) and P256 (aka secp256r1)
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EccCurveType {
    K256,
    P256,
}

impl EccCurveType {
    /// Return the length of a scalar (in bits)
    ///
    /// Scalar here refers to the byte size of an integer which has the range
    /// [0,z) where z is the group order.
    pub fn scalar_bits(&self) -> usize {
        match self {
            EccCurveType::K256 => 256,
            EccCurveType::P256 => 256,
        }
    }

    /// Return the length of a scalar (in bytes, rounded up)
    ///
    /// Scalar here refers to the byte size of an integer which has the range
    /// [0,z) where z is the group order.
    pub fn scalar_bytes(&self) -> usize {
        (self.scalar_bits() + 7) / 8
    }

    /// Return the length of the underlying field (in bits)
    pub fn field_bits(&self) -> usize {
        match self {
            EccCurveType::K256 => 256,
            EccCurveType::P256 => 256,
        }
    }

    /// Return the length of the underlying field (in bytes)
    ///
    /// If the field size is not an even multiple of 8 it is rounded up to the
    /// next byte size.
    pub fn field_bytes(&self) -> usize {
        // Round up to the nearest byte length
        (self.field_bits() + 7) / 8
    }

    /// Security level of the curve, in bits
    ///
    /// This must match the value specified in the hash2curve specification
    pub fn security_level(&self) -> usize {
        match self {
            EccCurveType::K256 => 128,
            EccCurveType::P256 => 128,
        }
    }

    /// Return a vector over all available curve types
    ///
    /// This is mostly useful for tests
    pub fn all() -> Vec<EccCurveType> {
        vec![EccCurveType::K256, EccCurveType::P256]
    }
}

impl fmt::Display for EccCurveType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let curve_name = match self {
            Self::K256 => "secp256k1",
            Self::P256 => "secp256r1",
        };

        write!(f, "{}", curve_name)
    }
}

/// An elliptic curve
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct EccCurve {
    curve: EccCurveType,
}

impl EccCurve {
    /// Return the curve type (eg EccCurveType::K256)
    pub fn curve_type(&self) -> EccCurveType {
        self.curve
    }

    /// Create a new curve
    pub fn new(curve: EccCurveType) -> Self {
        Self { curve }
    }

    /// Return a point which is the identity element on the curve
    pub fn neutral_element(&self) -> EccPoint {
        match self.curve {
            EccCurveType::K256 => EccPoint::K256(k256::ProjectivePoint::identity()),
            EccCurveType::P256 => EccPoint::P256(p256::ProjectivePoint::identity()),
        }
    }

    /// Return a point which is the "standard" generator on the curve
    pub fn generator_g(&self) -> ThresholdEcdsaResult<EccPoint> {
        match self.curve {
            EccCurveType::K256 => Ok(EccPoint::K256(k256::ProjectivePoint::generator())),
            EccCurveType::P256 => Ok(EccPoint::P256(p256::ProjectivePoint::generator())),
        }
    }

    /// Return a point which is unrelated to the standard generator on the curve
    ///
    /// The key point is that there is no known relation g*z = h as otherwise
    /// our commitment scheme would be insecure. Guarantee this relation is
    /// unknown by deriving h using a hash function.
    pub fn generator_h(&self) -> ThresholdEcdsaResult<EccPoint> {
        self.hash_to_point(
            "h".as_bytes(),
            format!("ic-crypto-tecdsa-{}-generator-h", self.curve).as_bytes(),
        )
    }

    /// Return a random scalar
    pub fn random_scalar<R: CryptoRng + RngCore>(
        &self,
        rng: &mut R,
    ) -> ThresholdEcdsaResult<EccScalar> {
        EccScalar::random(self.curve, rng)
    }

    /// Deserialize a scalar, returning an error if out of range
    ///
    /// The array must encode, in big-endian format, an integer in the
    /// range [0,n) where n is the order of the elliptic curve. The encoding
    /// must be zero-padded to be exactly curve_type().scalar_bytes() long
    pub fn deserialize_scalar(&self, bits: &[u8]) -> ThresholdEcdsaResult<EccScalar> {
        EccScalar::deserialize(self.curve, bits)
    }

    /// Hash an input to one or more a scalars
    pub fn hash_to_scalar(
        &self,
        count: usize,
        input: &[u8],
        domain_separator: &[u8],
    ) -> ThresholdEcdsaResult<Vec<EccScalar>> {
        hash2curve::hash_to_scalar(count, self.curve, input, domain_separator)
    }

    /// Deserialize a point, returning an error if invalid
    ///
    /// The point may be encoded in compressed or uncompressed SEC1 format.
    ///
    /// That is, either 0x04 followed by the encoding of x and y coordinates, or
    /// else either 0x02 or 0x03, followed by the encoding of the x coordinate,
    /// and the choice of 0x02 or 0x03 encodes the sign of y.
    pub fn deserialize_point(&self, bits: &[u8]) -> ThresholdEcdsaResult<EccPoint> {
        EccPoint::deserialize(self.curve, bits)
    }

    /// Hash an input (with domain separation) onto an elliptic curve point
    pub fn hash_to_point(
        &self,
        input: &[u8],
        domain_separator: &[u8],
    ) -> ThresholdEcdsaResult<EccPoint> {
        EccPoint::hash_to_point(self.curve_type(), input, domain_separator)
    }
}

impl fmt::Display for EccCurve {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.curve_type())
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum EccScalar {
    K256(k256::Scalar),
    P256(p256::Scalar),
}

impl fmt::Debug for EccScalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}(0x{})",
            self.curve_type(),
            hex::encode(self.serialize())
        )
    }
}

impl EccScalar {
    pub fn curve(&self) -> EccCurve {
        match self {
            Self::K256(_) => EccCurve::new(EccCurveType::K256),
            Self::P256(_) => EccCurve::new(EccCurveType::P256),
        }
    }

    pub fn curve_type(&self) -> EccCurveType {
        self.curve().curve_type()
    }

    /// Return the sum of two scalar values
    pub fn add(&self, other: &EccScalar) -> ThresholdEcdsaResult<Self> {
        match (self, other) {
            (Self::K256(s1), Self::K256(s2)) => Ok(Self::K256(s1.add(s2))),
            (Self::P256(s1), Self::P256(s2)) => Ok(Self::P256(s1.add(s2))),
            (_, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Return the difference of two scalar values
    pub fn sub(&self, other: &EccScalar) -> ThresholdEcdsaResult<Self> {
        use std::ops::Sub;
        match (self, other) {
            (Self::K256(s1), Self::K256(s2)) => Ok(Self::K256(s1.sub(s2))),
            (Self::P256(s1), Self::P256(s2)) => Ok(Self::P256(s1.sub(s2))),
            (_, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Return the product of two scalar values
    pub fn mul(&self, other: &EccScalar) -> ThresholdEcdsaResult<Self> {
        match (self, other) {
            (Self::K256(s1), Self::K256(s2)) => Ok(Self::K256(s1.mul(s2))),
            (Self::P256(s1), Self::P256(s2)) => Ok(Self::P256(s1.mul(s2))),
            (_, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Compute the modular inverse of Self
    ///
    /// Returns zero if self is equal to zero
    pub fn invert(&self) -> ThresholdEcdsaResult<Self> {
        match self {
            Self::K256(s) => {
                let inv = s.invert();
                if bool::from(inv.is_some()) {
                    Ok(EccScalar::K256(inv.unwrap()))
                } else {
                    Ok(EccScalar::zero(EccCurveType::K256))
                }
            }
            Self::P256(s) => {
                let inv = s.invert();
                if bool::from(inv.is_some()) {
                    Ok(EccScalar::P256(inv.unwrap()))
                } else {
                    Ok(EccScalar::zero(EccCurveType::P256))
                }
            }
        }
    }

    /// Serialize the scalar in SEC1 format
    ///
    /// In this context SEC1 format is just the big-endian fixed length encoding
    /// of the integer, with leading zero bytes included if necessary.
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Self::K256(s) => s.to_bytes().to_vec(),
            Self::P256(s) => s.to_bytes().to_vec(),
        }
    }

    /// Hash an input to a Scalar value
    pub fn hash_to_scalar(
        curve: EccCurveType,
        input: &[u8],
        domain_separator: &[u8],
    ) -> ThresholdEcdsaResult<Self> {
        let h = hash2curve::hash_to_scalar(1, curve, input, domain_separator)?;
        Ok(h[0])
    }

    /// Deserialize a SEC1 formatted scalar value
    pub fn deserialize(curve: EccCurveType, bits: &[u8]) -> ThresholdEcdsaResult<Self> {
        if bits.len() != curve.scalar_bytes() {
            return Err(ThresholdEcdsaError::InvalidScalar);
        }

        match curve {
            EccCurveType::K256 => {
                use k256::elliptic_curve::group::ff::PrimeField;
                let fb = k256::FieldBytes::from_slice(bits);
                let s = k256::Scalar::from_repr(*fb).ok_or(ThresholdEcdsaError::InvalidScalar)?;
                Ok(Self::K256(s))
            }
            EccCurveType::P256 => {
                use p256::elliptic_curve::group::ff::PrimeField;
                let fb = p256::FieldBytes::from_slice(bits);
                let s = p256::Scalar::from_repr(*fb).ok_or(ThresholdEcdsaError::InvalidScalar)?;
                Ok(Self::P256(s))
            }
        }
    }

    /// Compute the scalar from a larger value
    ///
    /// The input is allowed to be up to twice the length of a scalar. It is
    /// interpreted as a big-endian encoded integer, and reduced modulo the
    /// group order.
    pub fn from_bytes_wide(curve: EccCurveType, bits: &[u8]) -> ThresholdEcdsaResult<Self> {
        /*
        As the k256 and p256 crates are lacking a native function that reduces an
        input modulo the group order we have to synthesize it using other
        operations.

        Do so by splitting up the input into two parts each of which is at most
        scalar_len bytes long. Then compute s0*2^X + s1 where X depends on the
        order size.
        */
        let scalar_bytes = curve.scalar_bytes();

        if bits.len() > 2 * scalar_bytes {
            return Err(ThresholdEcdsaError::InvalidScalar);
        }

        let mut extended = vec![0; 2 * scalar_bytes];
        let offset = extended.len() - bits.len();
        extended[offset..].copy_from_slice(bits); // zero pad

        match curve {
            EccCurveType::K256 => {
                use k256::elliptic_curve::group::ff::Field;
                let fb0 = k256::FieldBytes::from_slice(&extended[..scalar_bytes]);
                let fb1 = k256::FieldBytes::from_slice(&extended[scalar_bytes..]);

                let mut s0 = k256::Scalar::from_bytes_reduced(fb0);
                let s1 = k256::Scalar::from_bytes_reduced(fb1);

                for _bit in 1..=scalar_bytes * 8 {
                    s0 = s0.double();
                }
                s0 += s1;

                Ok(Self::K256(s0))
            }
            EccCurveType::P256 => {
                let fb0 = p256::FieldBytes::from_slice(&extended[..scalar_bytes]);
                let fb1 = p256::FieldBytes::from_slice(&extended[scalar_bytes..]);

                let mut s0 = p256::Scalar::from_bytes_reduced(fb0);
                let s1 = p256::Scalar::from_bytes_reduced(fb1);

                for _bit in 1..=scalar_bytes * 8 {
                    s0 = s0.double();
                }
                s0 += s1;

                Ok(Self::P256(s0))
            }
        }
    }

    /// Generate a random scalar
    pub fn random<R: CryptoRng + RngCore>(
        curve: EccCurveType,
        rng: &mut R,
    ) -> ThresholdEcdsaResult<Self> {
        // Use rejection sampling to avoid biasing the output

        let mut buf = vec![0u8; curve.scalar_bytes()];

        loop {
            rng.fill_bytes(&mut buf);
            if let Ok(scalar) = Self::deserialize(curve, &buf) {
                return Ok(scalar);
            }
        }
    }

    /// Return true iff self is equal to zero
    pub fn is_zero(&self) -> bool {
        match self {
            Self::K256(s) => bool::from(s.is_zero()),
            Self::P256(s) => bool::from(s.is_zero()),
        }
    }

    /// Negation within the scalar field
    ///
    /// Effectively this returns p - self where p is the primefield
    /// order of the elliptic curve group, and the subtraction occurs
    /// within the integers modulo the curve order.
    pub fn negate(&self) -> ThresholdEcdsaResult<Self> {
        match self {
            Self::K256(s) => Ok(Self::K256(s.negate())),
            Self::P256(s) => {
                use std::ops::Neg;
                Ok(Self::P256(s.neg()))
            }
        }
    }

    /// Return the scalar 0
    ///
    /// Since scalars are simply integers modulo some prime this is
    /// just plain 0.
    pub fn zero(curve: EccCurveType) -> Self {
        match curve {
            EccCurveType::K256 => Self::K256(k256::Scalar::zero()),
            EccCurveType::P256 => Self::P256(p256::Scalar::zero()),
        }
    }

    /// Return the scalar 1
    ///
    /// Since scalars are simply integers modulo some prime this is
    /// just plain 1.
    pub fn one(curve: EccCurveType) -> Self {
        match curve {
            EccCurveType::K256 => Self::K256(k256::Scalar::one()),
            EccCurveType::P256 => Self::P256(p256::Scalar::one()),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum EccPoint {
    K256(k256::ProjectivePoint),
    P256(p256::ProjectivePoint),
}

impl EccPoint {
    pub fn curve(&self) -> EccCurve {
        match self {
            Self::K256(_) => EccCurve::new(EccCurveType::K256),
            Self::P256(_) => EccCurve::new(EccCurveType::P256),
        }
    }

    pub fn curve_type(&self) -> EccCurveType {
        self.curve().curve_type()
    }

    /// Hash an input to a valid elliptic curve point
    ///
    /// This uses the techniques described in the hash to curve internet draft
    /// https://www.ietf.org/archive/id/draft-irtf-cfrg-hash-to-curve-12.txt
    ///
    /// Only the random oracle ("RO") variant is supplied as the non-uniform
    /// ("NU") variant is possibly insecure to use in some contexts. Only curves
    /// with extension degree of 1 are currently supported.
    pub fn hash_to_point(
        curve: EccCurveType,
        input: &[u8],
        domain_separator: &[u8],
    ) -> ThresholdEcdsaResult<Self> {
        hash2curve::hash2curve_ro(curve, input, domain_separator)
    }

    /// Create a point from two field elements
    ///
    /// The (x,y) pair must satisfy the curve equation
    pub fn from_field_elems(
        x: &EccFieldElement,
        y: &EccFieldElement,
    ) -> ThresholdEcdsaResult<Self> {
        if x.curve_type() != y.curve_type() {
            return Err(ThresholdEcdsaError::CurveMismatch);
        }

        let curve = x.curve_type();
        let x_bytes = x.as_bytes();
        let y_bytes = y.as_bytes();
        let mut encoded = Vec::with_capacity(1 + x_bytes.len() + y_bytes.len());
        encoded.push(0x04); // uncompressed
        encoded.extend_from_slice(&x_bytes);
        encoded.extend_from_slice(&y_bytes);
        Self::deserialize(curve, &encoded)
    }

    /// Add two elliptic curve points
    pub fn add_points(&self, other: &Self) -> ThresholdEcdsaResult<Self> {
        match (self, other) {
            (Self::K256(pt1), Self::K256(pt2)) => Ok(Self::K256(pt1 + pt2)),
            (Self::P256(pt1), Self::P256(pt2)) => Ok(Self::P256(pt1 + pt2)),
            (_, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Perform point*scalar multiplication
    pub fn scalar_mul(&self, scalar: &EccScalar) -> ThresholdEcdsaResult<Self> {
        match (self, scalar) {
            (Self::K256(pt), EccScalar::K256(s)) => Ok(Self::K256(pt * s)),
            (Self::P256(pt), EccScalar::P256(s)) => Ok(Self::P256(pt * s)),
            (_, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Return self * scalar1 + other * scalar2
    pub fn mul_points(
        &self,
        scalar1: &EccScalar,
        other: &Self,
        scalar2: &EccScalar,
    ) -> ThresholdEcdsaResult<Self> {
        match (self, scalar1, other, scalar2) {
            (Self::K256(pt1), EccScalar::K256(s1), Self::K256(pt2), EccScalar::K256(s2)) => {
                Ok(Self::K256(k256::lincomb(pt1, s1, pt2, s2)))
            }

            (Self::P256(pt1), EccScalar::P256(s1), Self::P256(pt2), EccScalar::P256(s2)) => {
                Ok(Self::P256(pt1 * s1 + pt2 * s2))
            }
            (_, _, _, _) => Err(ThresholdEcdsaError::CurveMismatch),
        }
    }

    /// Serialize a point in compressed form
    ///
    /// The output is in SEC1 format, and will be 1 header byte
    /// followed by a single field element, which for K256 and P256 is
    /// 32 bytes long.
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Self::K256(pt) => {
                use k256::elliptic_curve::group::GroupEncoding;
                pt.to_affine().to_bytes().to_vec()
            }
            Self::P256(pt) => {
                use p256::elliptic_curve::group::GroupEncoding;
                pt.to_affine().to_bytes().to_vec()
            }
        }
    }

    /// Serialize a point in uncompressed form
    ///
    /// The output is in SEC1 format, and will be 1 header byte
    /// followed by a two field elements, which for K256 and P256 is
    /// 32 bytes long each.
    fn serialize_uncompressed(&self) -> Vec<u8> {
        let compress = false;

        match self {
            Self::K256(pt) => {
                use k256::elliptic_curve::sec1::ToEncodedPoint;
                pt.to_affine()
                    .to_encoded_point(compress)
                    .as_bytes()
                    .to_vec()
            }
            Self::P256(pt) => {
                use p256::elliptic_curve::sec1::ToEncodedPoint;
                pt.to_affine()
                    .to_encoded_point(compress)
                    .as_bytes()
                    .to_vec()
            }
        }
    }

    /// Return the affine X coordinate of this point
    pub fn affine_x(&self) -> ThresholdEcdsaResult<EccFieldElement> {
        let curve_type = self.curve_type();
        let field_bytes = curve_type.field_bytes();
        let z = self.serialize_uncompressed();
        EccFieldElement::from_bytes(curve_type, &z[1..field_bytes + 1])
    }

    /// Return the affine Y coordinate of this point
    pub fn affine_y(&self) -> ThresholdEcdsaResult<EccFieldElement> {
        let curve_type = self.curve_type();
        let field_bytes = curve_type.field_bytes();
        let z = self.serialize_uncompressed();
        EccFieldElement::from_bytes(curve_type, &z[1 + field_bytes..])
    }

    /// Deserialize a point. Either compressed or uncompressed points are
    /// accepted.
    pub fn deserialize(curve: EccCurveType, bits: &[u8]) -> ThresholdEcdsaResult<Self> {
        match curve {
            EccCurveType::K256 => {
                use k256::elliptic_curve::sec1::FromEncodedPoint;
                let ept = k256::EncodedPoint::from_bytes(bits)
                    .map_err(|_| ThresholdEcdsaError::InvalidPoint)?;
                let apt = k256::AffinePoint::from_encoded_point(&ept);

                match apt {
                    Some(apt) => Ok(Self::K256(k256::ProjectivePoint::from(apt))),
                    None => Err(ThresholdEcdsaError::InvalidPoint),
                }
            }
            EccCurveType::P256 => {
                use p256::elliptic_curve::sec1::FromEncodedPoint;
                let ept = p256::EncodedPoint::from_bytes(bits)
                    .map_err(|_| ThresholdEcdsaError::InvalidPoint)?;
                let apt = p256::AffinePoint::from_encoded_point(&ept);
                match apt {
                    Some(apt) => Ok(Self::P256(p256::ProjectivePoint::from(apt))),
                    None => Err(ThresholdEcdsaError::InvalidPoint),
                }
            }
        }
    }
}
