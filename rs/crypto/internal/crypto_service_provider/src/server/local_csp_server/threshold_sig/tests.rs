#![allow(clippy::unwrap_used)]
//! Tests for threshold signature implementations

use crate::api::ThresholdSignatureCspClient;
use crate::secret_key_store::test_utils::TempSecretKeyStore;
use crate::secret_key_store::SecretKeyStore;
use crate::types::{CspPublicCoefficients, CspSignature, ThresBls12_381_Signature};
use crate::Csp;
use ic_crypto_internal_threshold_sig_bls12381::test_utils::select_n;
use ic_types::crypto::{AlgorithmId, KeyId};
use ic_types::{NodeIndex, NumberOfNodes, Randomness};
use proptest::prelude::*;
use rand::{CryptoRng, Rng, SeedableRng};
use rand_chacha::ChaChaRng;
use strum::IntoEnumIterator;

pub mod util {
    use super::*;
    use crate::api::CspThresholdSignError;
    use crate::server::api::ThresholdSignatureCspServer;
    use crate::server::local_csp_server::LocalCspServer;
    use ic_crypto_internal_threshold_sig_bls12381::types::public_coefficients::conversions::try_number_of_nodes_from_csp_pub_coeffs;

    /// Test that a set of threshold signatures behaves correctly.
    ///
    /// This assumes that a set of keys has been provided and verifies that:
    /// * If the threshold signatures are used correctly, signatures verify.
    /// * If incorrect values are provided at any stage, relevant methods fail.
    /// Note: We assume that all signers have been dealt keys but disqualify
    /// some as part of the test.
    ///
    /// # Arguments
    /// * `public_coefficients` is the public part of the threshold key.  This
    ///   can be used by third parties to verify individual and combined
    ///   threshold signatures.
    /// * `signers` is the entities with the secret threshold keys.  For each
    ///   signer the array contains a reference to the signer's crypto service
    ///   provider, which contains the secret key, and the key identifier.
    /// * `seed` is a source of randomness.
    /// * `message` is a test message.
    pub fn test_threshold_signatures<R: Rng + CryptoRng, S: SecretKeyStore, C: SecretKeyStore>(
        public_coefficients: &CspPublicCoefficients,
        signers: &[(&LocalCspServer<R, S, C>, KeyId)],
        seed: Randomness,
        message: &[u8],
    ) {
        let mut rng = ChaChaRng::from_seed(seed.get());
        let threshold = try_number_of_nodes_from_csp_pub_coeffs(public_coefficients)
            .expect("Intolerable number of nodes");
        let incorrect_message = [&b"pound of flesh"[..], message].concat();

        // Signatures can be generated correctly:
        let signatures: Result<Vec<CspSignature>, CspThresholdSignError> = signers
            .iter()
            .map(|(csp_server, key_id)| {
                csp_server.threshold_sign(AlgorithmId::ThresBls12_381, message, *key_id)
            })
            .collect();
        let signatures = signatures.expect("Signing failed");
        {
            // But:
            // * Signatures cannot be generated with an incorrect AlgorithmId:
            for algorithm_id in AlgorithmId::iter() {
                if algorithm_id != AlgorithmId::ThresBls12_381 {
                    if let Some((csp_server, key_id)) = signers.get(0) {
                        assert!(
                            csp_server
                                .threshold_sign(algorithm_id, message, *key_id)
                                .is_err(),
                            "Managed to threshold sign with algorithm ID {:?}",
                            algorithm_id
                        )
                    }
                }
            }
            //
            // * Signatures cannot be generated with an incorrect key_id:
            if let Some((csp_server, _key_id)) = signers.get(0) {
                let wrong_key_id = KeyId::from(rng.gen::<[u8; 32]>());
                let mut key_ids = signers.iter().map(|(_, key_id)| *key_id);

                assert!(
                    !key_ids.any(|x| x == wrong_key_id),
                    "Bad RNG: A randomly generated KeyId was in the list of keys"
                );
                assert!(
                    csp_server
                        .threshold_sign(AlgorithmId::ThresBls12_381, message, wrong_key_id)
                        .is_err(),
                    "A randomly generated key_id managed to sign"
                );
            }
        }

        // Verify each individual signature:
        let verifier = {
            let dummy_key_store = TempSecretKeyStore::new();
            let csprng = ChaChaRng::from_seed(rng.gen::<[u8; 32]>());
            Csp::of(csprng, dummy_key_store)
        };
        for (index, signature) in signatures.iter().enumerate() {
            let public_key = match verifier.threshold_individual_public_key(
                AlgorithmId::ThresBls12_381,
                index as NodeIndex,
                (*public_coefficients).clone(),
            ) {
                Ok(public_key) => public_key,
                Err(error) => panic!("Could not calculate individual public key: {:?}", error),
            };

            // Correct values validate:
            assert_eq!(
                verifier.threshold_verify_individual_signature(
                    AlgorithmId::ThresBls12_381,
                    message,
                    signature.clone(),
                    public_key
                ),
                Ok(()),
                "Individual signature failed verification for signatory number {}/{}",
                index,
                signers.len()
            );

            // Mismatched public key fails to validate:
            if threshold > NumberOfNodes::from(1)
            // Otherwise all the secret keys are the same
            {
                // Here we mix up signatures and keys so that we are validating valid signatures
                // with valid public keys but the two are not matched.  We expect to see that if
                // the signature or (equivalently) the public key is wrong, validation fails.
                let wrong_index = (index + 1) % signers.len();
                let wrong_public_key = verifier
                    .threshold_individual_public_key(
                        AlgorithmId::ThresBls12_381,
                        wrong_index as NodeIndex,
                        public_coefficients.clone(),
                    )
                    .expect("Should be able to compute the wrong public key.");
                assert!(
                            verifier.threshold_verify_individual_signature(AlgorithmId::ThresBls12_381, message, signature.clone(), wrong_public_key).is_err(),
                            "Individual signature verification accepted incorrect signatory {} instead of {}/{}",
                            wrong_index,
                            index,
                            signers.len()
                        );
            }
            // Incorrect message fails to validate:
            if threshold > NumberOfNodes::from(0)
            // threshold > 0 otherwise all signatures are the same
            {
                assert!(
                            verifier.threshold_verify_individual_signature(AlgorithmId::ThresBls12_381, &incorrect_message, signature.clone(), public_key).is_err(),
                            "Individual signature verification accepted incorrect message '{:?}' instead of '{:?}'",
                            &incorrect_message,
                            message
                        );
            }
        }

        // Combine a random subset of signatures:
        let signature_selection = select_n(seed, threshold, &signatures);
        let signature = verifier
            .threshold_combine_signatures(
                AlgorithmId::ThresBls12_381,
                &signature_selection,
                public_coefficients.clone(),
            )
            .expect("Failed to combine signatures");

        // Correct values validate:
        assert_eq!(
            verifier.threshold_verify_combined_signature(
                AlgorithmId::ThresBls12_381,
                message,
                signature.clone(),
                public_coefficients.clone()
            ),
            Ok(())
        );

        // Incorrect values are rejected:
        if threshold > NumberOfNodes::from(0) {
            // threshold > 0, otherwise all signatures are the same.
            // Incorrect message:
            assert!(verifier
                .threshold_verify_combined_signature(
                    AlgorithmId::ThresBls12_381,
                    &incorrect_message,
                    signature.clone(),
                    public_coefficients.clone()
                )
                .is_err());
            // Incorrect signature:
            let incorrect_signature = {
                if let CspSignature::ThresBls12_381(ThresBls12_381_Signature::Combined(
                    mut signature_bytes,
                )) = signature
                {
                    signature_bytes.0[0] = !signature_bytes.0[0];
                    CspSignature::ThresBls12_381(ThresBls12_381_Signature::Combined(
                        signature_bytes,
                    ))
                } else {
                    unreachable!()
                }
            };
            assert!(verifier
                .threshold_verify_combined_signature(
                    AlgorithmId::ThresBls12_381,
                    message,
                    incorrect_signature,
                    public_coefficients.clone()
                )
                .is_err());
        }
        if threshold > NumberOfNodes::from(1) {
            // Otherwise all secret keys are the same.
            let some_individual_signature = signatures[0].clone();
            assert!(
                verifier.threshold_verify_combined_signature(
                    AlgorithmId::ThresBls12_381,
                    message,
                    some_individual_signature.clone(),
                    public_coefficients.clone()
                )
                .is_err(),
                "Combined signature verification passed with an individual signature: Used signature: {:?} Correct signature: {:?}",
                some_individual_signature,
                signature
            );
        }
    }

    /// Verify that the basic key generation behaves correctly:
    /// * Incorrect keygen arguments return an error:
    ///   * If the threshold is higher than the number of signers, keygen fails.
    /// * Correct keygen arguments yield keys that behave correctly with regards
    ///   to signing and verification.
    pub fn test_threshold_scheme_with_basic_keygen(seed: Randomness, message: &[u8]) {
        let mut rng = ChaChaRng::from_seed(seed.get());
        let threshold = NumberOfNodes::from(rng.gen_range(0, 10));
        let number_of_signers = NumberOfNodes::from(rng.gen_range(0, 10));

        let csp_server = {
            let key_store = TempSecretKeyStore::new();
            let csprng = ChaChaRng::from_seed(rng.gen::<[u8; 32]>());
            LocalCspServer::new_for_test(csprng, key_store)
        };

        match csp_server.threshold_keygen_for_test(
            AlgorithmId::ThresBls12_381,
            threshold,
            &vec![true; number_of_signers.get() as usize],
        ) {
            Ok((public_coefficients, key_ids)) => {
                assert!(
                    number_of_signers >= threshold,
                    "Generated keys even though the threshold is too high"
                );

                let signers: Vec<_> = key_ids
                    .iter()
                    .map(|key_id_maybe| (&csp_server, key_id_maybe.expect("Missing key")))
                    .collect();

                test_threshold_signatures(
                    &public_coefficients,
                    &signers,
                    Randomness::from(rng.gen::<[u8; 32]>()),
                    message,
                );
            }
            Err(_) => assert!(number_of_signers < threshold, "Failed to generate keys"),
        }
    }
}

// Slow tests
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_threshold_scheme_with_basic_keygen(seed: [u8;32], message in proptest::collection::vec(any::<u8>(), 0..100)) {
        util::test_threshold_scheme_with_basic_keygen(Randomness::from(seed), &message);
    }
}
