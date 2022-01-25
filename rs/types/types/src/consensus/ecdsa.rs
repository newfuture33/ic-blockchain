//! Defines types used for threshold ECDSA key generation.

// TODO: Remove once we have implemented the functionality
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    convert::TryFrom,
    ops::{Deref, DerefMut},
};

use crate::consensus::{BasicSignature, Block, BlockPayload, MultiSignature, MultiSignatureShare};
use crate::crypto::{
    canister_threshold_sig::idkg::{
        IDkgDealing, IDkgTranscript, IDkgTranscriptId, IDkgTranscriptParams, IDkgTranscriptType,
    },
    canister_threshold_sig::{
        PreSignatureQuadruple, ThresholdEcdsaCombinedSignature, ThresholdEcdsaSigInputs,
        ThresholdEcdsaSigShare,
    },
    CryptoHashOf, Signed, SignedBytesWithoutDomainSeparator,
};
use crate::{Height, NodeId};
use phantom_newtype::Id;

pub type EcdsaSignature = ThresholdEcdsaCombinedSignature;

/// The payload information necessary for ECDSA threshold signatures, that is
/// published on every consensus round. It represents the current state of
/// the protocol since the summary block.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EcdsaDataPayload {
    /// Signatures that we agreed upon in this round.
    pub signature_agreements: BTreeMap<RequestId, EcdsaSignature>,

    /// The `RequestIds` for which we are currently generating signatures.
    pub ongoing_signatures: BTreeMap<RequestId, ThresholdEcdsaSigInputs>,

    /// ECDSA transcript quadruples that we can use to create ECDSA signatures.
    pub available_quadruples: BTreeMap<QuadrupleId, PreSignatureQuadruple>,

    /// Ecdsa Quadruple in creation.
    pub quadruples_in_creation: BTreeMap<QuadrupleId, QuadrupleInCreation>,

    /// Next TranscriptId that is incremented after creating a new transcript.
    pub next_unused_transcript_id: IDkgTranscriptId,

    /// Progress of creating the next ECDSA key transcript.
    pub next_key_transcript_creation: Option<KeyTranscriptCreation>,
}

/// The creation of an ecdsa key transcript goes through one of the two paths below:
/// 1. RandomTranscript -> ReshareOfMasked -> Created
/// 2. ReshareOfUnmasked -> Created
///
/// The initial bootstrap will start with an empty 'EcdsaSummaryPayload', and then
/// we'll go through the first path to create the key transcript.
///
/// After the initial key transcript is created, we will be able to create the first
/// 'EcdsaSummaryPayload' by carrying over the key transcript, which will be carried
/// over to the next DKG interval if there is no node membership change.
///
/// If in the future there is a membership change, we will create a new key transcript
/// by going through the second path above. Then the switch-over will happen at
/// the next 'EcdsaSummaryPayload'.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyTranscriptCreation {
    // Configuration to create initial random transcript.
    RandomTranscriptParams(RandomTranscriptParams),
    // Configuration to create initial key transcript by resharing the random transcript.
    ReshareOfMaskedParams(ReshareOfMaskedParams),
    // Configuration to create next key transcript by resharing the current key transcript.
    ReshareOfUnmaskedParams(ReshareOfUnmaskedParams),
    // Created
    Created(UnmaskedTranscript),
}

impl EcdsaDataPayload {
    /// Return an iterator of all transcript configs that have no matching
    /// results yet.
    pub fn iter_transcript_configs_in_creation(
        &self,
    ) -> Box<dyn Iterator<Item = &IDkgTranscriptParams> + '_> {
        let iter =
            self.next_key_transcript_creation
                .iter()
                .filter_map(|transcript| match transcript {
                    KeyTranscriptCreation::RandomTranscriptParams(x) => Some(x),
                    KeyTranscriptCreation::ReshareOfMaskedParams(x) => Some(x),
                    KeyTranscriptCreation::ReshareOfUnmaskedParams(x) => Some(x),
                    KeyTranscriptCreation::Created(_) => None,
                });
        Box::new(
            self.quadruples_in_creation
                .iter()
                .map(|(_, quadruple)| quadruple.iter_transcript_configs_in_creation())
                .flatten()
                .chain(iter),
        )
    }
}

/// The payload information necessary for ECDSA threshold signatures, that is
/// published on summary blocks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EcdsaSummaryPayload {
    /// The `RequestIds` for which we are currently generating signatures.
    pub ongoing_signatures: BTreeMap<RequestId, ThresholdEcdsaSigInputs>,

    /// The ECDSA key transcript used for the corresponding interval.
    pub current_key_transcript: UnmaskedTranscript,

    /// ECDSA transcript quadruples that we can use to create ECDSA signatures.
    pub available_quadruples: BTreeMap<QuadrupleId, PreSignatureQuadruple>,

    /// Next TranscriptId that is incremented after creating a new transcript.
    pub next_unused_transcript_id: IDkgTranscriptId,
}

#[derive(
    Copy, Clone, Default, Debug, PartialOrd, Ord, PartialEq, Eq, Serialize, Deserialize, Hash,
)]
pub struct QuadrupleId(pub usize);

impl QuadrupleId {
    pub fn increment(self) -> QuadrupleId {
        QuadrupleId(self.0 + 1)
    }
}

/// The ECDSA artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum EcdsaMessage {
    EcdsaSignedDealing(EcdsaSignedDealing),
    EcdsaDealingSupport(EcdsaDealingSupport),
    EcdsaSigShare(EcdsaSigShare),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub enum EcdsaMessageHash {
    EcdsaSignedDealing(CryptoHashOf<EcdsaSignedDealing>),
    EcdsaDealingSupport(CryptoHashOf<EcdsaDealingSupport>),
    EcdsaSigShare(CryptoHashOf<EcdsaSigShare>),
}

/// The dealing content generated by a dealer
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct EcdsaDealing {
    /// Height of the finalized block that requested the transcript
    pub requested_height: Height,

    /// The crypto dealing
    /// TODO: dealers should send the BasicSigned<> dealing
    pub idkg_dealing: IDkgDealing,
}

impl SignedBytesWithoutDomainSeparator for EcdsaDealing {
    fn as_signed_bytes_without_domain_separator(&self) -> Vec<u8> {
        serde_cbor::to_vec(&self).unwrap()
    }
}

/// The signed dealing sent by dealers
pub type EcdsaSignedDealing = Signed<EcdsaDealing, BasicSignature<EcdsaDealing>>;

impl EcdsaSignedDealing {
    pub fn get(&self) -> &EcdsaDealing {
        &self.content
    }
}

impl SignedBytesWithoutDomainSeparator for EcdsaSignedDealing {
    fn as_signed_bytes_without_domain_separator(&self) -> Vec<u8> {
        serde_cbor::to_vec(&self).unwrap()
    }
}

/// TODO: EcdsaDealing can be big, consider sending only the signature
/// as part of the shares
/// The individual signature share in support of a dealing
pub type EcdsaDealingSupport = Signed<EcdsaDealing, MultiSignatureShare<EcdsaDealing>>;

/// The multi-signature verified dealing
pub type EcdsaVerifiedDealing = Signed<EcdsaDealing, MultiSignature<EcdsaDealing>>;

/// The ECDSA signature share
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct EcdsaSigShare {
    /// Height of the finalized block that requested the signature
    pub requested_height: Height,

    /// The node that signed the share
    pub signer_id: NodeId,

    /// The request this signature share belongs to
    pub request_id: RequestId,

    /// The signature share
    pub share: ThresholdEcdsaSigShare,
}

/// The final output of the transcript creation sequence
pub type EcdsaTranscript = IDkgTranscript;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EcdsaMessageAttribute {
    EcdsaSignedDealing(Height),
    EcdsaDealingSupport(Height),
    EcdsaSigShare(Height),
}

impl From<&EcdsaMessage> for EcdsaMessageAttribute {
    fn from(msg: &EcdsaMessage) -> EcdsaMessageAttribute {
        match msg {
            EcdsaMessage::EcdsaSignedDealing(dealing) => {
                EcdsaMessageAttribute::EcdsaSignedDealing(dealing.content.requested_height)
            }
            EcdsaMessage::EcdsaDealingSupport(support) => {
                EcdsaMessageAttribute::EcdsaDealingSupport(support.content.requested_height)
            }
            EcdsaMessage::EcdsaSigShare(share) => {
                EcdsaMessageAttribute::EcdsaSigShare(share.requested_height)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Unmasked<T>(T);
pub type UnmaskedTranscript = Unmasked<IDkgTranscript>;

impl UnmaskedTranscript {
    pub fn transcript_id(&self) -> IDkgTranscriptId {
        self.0.transcript_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct TranscriptCastError {
    pub transcript_id: IDkgTranscriptId,
    pub from_type: IDkgTranscriptType,
    pub expected_type: &'static str,
}

impl TryFrom<IDkgTranscript> for UnmaskedTranscript {
    type Error = TranscriptCastError;
    fn try_from(transcript: IDkgTranscript) -> Result<Self, Self::Error> {
        match transcript.transcript_type {
            IDkgTranscriptType::Unmasked(_) => Ok(Unmasked(transcript)),
            _ => Err(TranscriptCastError {
                transcript_id: transcript.transcript_id,
                from_type: transcript.transcript_type,
                expected_type: "Unmasked",
            }),
        }
    }
}

impl From<UnmaskedTranscript> for IDkgTranscript {
    fn from(unmasked: UnmaskedTranscript) -> Self {
        unmasked.0
    }
}

impl<T> Deref for Unmasked<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Unmasked<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Masked<T>(T);
pub type MaskedTranscript = Masked<IDkgTranscript>;

impl MaskedTranscript {
    pub fn transcript_id(&self) -> IDkgTranscriptId {
        self.0.transcript_id
    }
}

impl TryFrom<IDkgTranscript> for MaskedTranscript {
    type Error = TranscriptCastError;
    fn try_from(transcript: IDkgTranscript) -> Result<Self, Self::Error> {
        match transcript.transcript_type {
            IDkgTranscriptType::Masked(_) => Ok(Masked(transcript)),
            _ => Err(TranscriptCastError {
                transcript_id: transcript.transcript_id,
                from_type: transcript.transcript_type,
                expected_type: "Unmasked",
            }),
        }
    }
}

impl From<MaskedTranscript> for IDkgTranscript {
    fn from(masked: MaskedTranscript) -> IDkgTranscript {
        masked.0
    }
}

impl<T> Deref for Masked<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Masked<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

pub type ResharingTranscript = Masked<IDkgTranscript>;
pub type MultiplicationTranscript = Masked<IDkgTranscript>;

pub type RandomTranscriptParams = IDkgTranscriptParams;
pub type ReshareOfMaskedParams = IDkgTranscriptParams;
pub type ReshareOfUnmaskedParams = IDkgTranscriptParams;
pub type MaskedTimesMaskedParams = IDkgTranscriptParams;
pub type UnmaskedTimesMaskedParams = IDkgTranscriptParams;

pub struct RequestIdTag;
pub type RequestId = Id<RequestIdTag, Vec<u8>>;

#[allow(missing_docs)]
/// Mock module of the crypto types that are needed by consensus for threshold
/// ECDSA generation. These types should be replaced by the real Types once they
/// are available.
pub mod ecdsa_crypto_mock {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
    pub struct EcdsaComplaint;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
    pub struct EcdsaOpening;
}

// The ECDSA summary.
pub type Summary = Option<EcdsaSummaryPayload>;

pub type Payload = Option<EcdsaDataPayload>;

/// ECDSA Quadruple in creation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuadrupleInCreation {
    pub kappa_config: RandomTranscriptParams,
    pub kappa_masked: Option<MaskedTranscript>,

    pub lambda_config: RandomTranscriptParams,
    pub lambda_masked: Option<MaskedTranscript>,

    pub unmask_kappa_config: Option<ReshareOfMaskedParams>,
    pub kappa_unmasked: Option<UnmaskedTranscript>,

    pub key_times_lambda_config: Option<UnmaskedTimesMaskedParams>,
    pub key_times_lambda: Option<MaskedTranscript>,

    pub kappa_times_lambda_config: Option<UnmaskedTimesMaskedParams>,
    pub kappa_times_lambda: Option<MaskedTranscript>,
}

impl QuadrupleInCreation {
    /// Initialization with the given random param pair.
    pub fn new(
        kappa_config: RandomTranscriptParams,
        lambda_config: RandomTranscriptParams,
    ) -> Self {
        QuadrupleInCreation {
            kappa_config,
            kappa_masked: None,
            lambda_config,
            lambda_masked: None,
            unmask_kappa_config: None,
            kappa_unmasked: None,
            key_times_lambda_config: None,
            key_times_lambda: None,
            kappa_times_lambda_config: None,
            kappa_times_lambda: None,
        }
    }
}

impl QuadrupleInCreation {
    /// Return an iterator of all transcript configs that have no matching
    /// results yet.
    pub fn iter_transcript_configs_in_creation(
        &self,
    ) -> Box<dyn Iterator<Item = &IDkgTranscriptParams> + '_> {
        let mut params = Vec::new();
        if self.kappa_masked.is_none() {
            params.push(&self.kappa_config)
        }
        if self.lambda_masked.is_none() {
            params.push(&self.lambda_config)
        }
        if let (Some(config), None) = (&self.unmask_kappa_config, &self.kappa_unmasked) {
            params.push(config)
        }
        if let (Some(config), None) = (&self.key_times_lambda_config, &self.key_times_lambda) {
            params.push(config)
        }
        if let (Some(config), None) = (&self.kappa_times_lambda_config, &self.kappa_times_lambda) {
            params.push(config)
        }
        Box::new(params.into_iter())
    }
}

/// Wrapper to access the ECDSA related info from the blocks.
pub trait EcdsaBlockReader {
    /// Returns the height of the block
    fn height(&self) -> Height;

    /// Returns the transcripts requested by the block.
    fn requested_transcripts(&self) -> Box<dyn Iterator<Item = &IDkgTranscriptParams> + '_>;

    /// Returns the signatures requested by the block.
    fn requested_signatures(
        &self,
    ) -> Box<dyn Iterator<Item = (&RequestId, &ThresholdEcdsaSigInputs)> + '_>;

    // TODO: APIs for completed transcripts, etc.
}

pub struct EcdsaBlockReaderImpl {
    height: Height,
    ecdsa_payload: Option<EcdsaDataPayload>,
}

impl EcdsaBlockReaderImpl {
    pub fn new(block: Block) -> Self {
        let height = block.height;
        let ecdsa_payload = if !block.payload.is_summary() {
            BlockPayload::from(block.payload).into_data().ecdsa
        } else {
            None
        };
        Self {
            height,
            ecdsa_payload,
        }
    }
}

impl EcdsaBlockReader for EcdsaBlockReaderImpl {
    fn height(&self) -> Height {
        self.height
    }

    fn requested_transcripts(&self) -> Box<dyn Iterator<Item = &IDkgTranscriptParams> + '_> {
        self.ecdsa_payload
            .as_ref()
            .map_or(Box::new(std::iter::empty()), |ecdsa_payload| {
                ecdsa_payload.iter_transcript_configs_in_creation()
            })
    }

    fn requested_signatures(
        &self,
    ) -> Box<dyn Iterator<Item = (&RequestId, &ThresholdEcdsaSigInputs)> + '_> {
        self.ecdsa_payload
            .as_ref()
            .map_or(Box::new(std::iter::empty()), |ecdsa_payload| {
                Box::new(ecdsa_payload.ongoing_signatures.iter())
            })
    }
}
