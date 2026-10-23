use crate::error::SignError;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer as SolanaSigner;

pub trait WalletSigner: Send + Sync {
    fn pubkey(&self) -> Pubkey;
    fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignError>;
}

pub struct KeypairSigner {
    keypair: Keypair,
}

impl KeypairSigner {
    pub fn from_keypair(keypair: Keypair) -> Self {
        Self { keypair }
    }

    pub fn from_seed(seed: &[u8]) -> Result<Self, SignError> {
        let secret: [u8; 32] = seed
            .try_into()
            .map_err(|_| SignError::Message("keypair seed must be 32 bytes".into()))?;
        Ok(Self {
            keypair: Keypair::new_from_array(secret),
        })
    }

    pub fn new_random() -> Self {
        Self {
            keypair: Keypair::new(),
        }
    }

    pub fn seed_bytes(&self) -> [u8; 32] {
        *self.keypair.secret_bytes()
    }
}

impl WalletSigner for KeypairSigner {
    fn pubkey(&self) -> Pubkey {
        SolanaSigner::pubkey(&self.keypair)
    }

    fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignError> {
        Ok(SolanaSigner::sign_message(&self.keypair, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keypair_signs_what_its_address_verifies() {
        let signer = KeypairSigner::new_random();
        let pk = signer.pubkey();
        let sig = signer.try_sign_message(b"hello").unwrap();
        assert!(sig.verify(pk.as_ref(), b"hello"));
        assert_eq!(
            KeypairSigner::from_seed(&signer.seed_bytes())
                .unwrap()
                .pubkey(),
            pk
        );
    }
}
