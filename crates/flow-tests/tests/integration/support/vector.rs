//! Vector wallets on the local chain, and the app's path for them: a site's
//! transaction, the rebuild, signing on this device or another, the chain.

use super::node::Node;
use super::{logs, Chain, Landed, SOL};
use solana_pubkey::Pubkey;
use std::future::Future;
use std::sync::Mutex;
use wallet_core::adapt::{plan_adaptation, Adapted};
use wallet_core::protocol::{decode_object, encode_object, QrObject, Signed as SignedObject};
use wallet_core::relay::{
    accept_signature_in, vector_signature_objects, AuthContext, VectorContext,
};
use wallet_core::vector::{
    address, initialize, keys_to_sign_with, local_vector_keys, FloorsFrom, LeafMarks, SchemeId,
    VectorAccount, XmssRetryCache, PROGRAM_ID,
};
use wallet_core::{
    decode_v1_transaction, Authority, ComputeBudgetSpec, Instruction, KeypairSigner, MemoryStore,
    SecretBytes, Vault, WalletId, WalletRecord, WalletSigner,
};
use wallet_flows::compile::compile_fresh;
use wallet_flows::sign::{sign_with, Signed, VectorSigning};

pub fn run<T>(flow: impl Future<Output = T>) -> T {
    pollster::block_on(flow)
}

/// An Ed25519 key whose Vector account exists on the chain and holds one SOL.
pub struct Account {
    pub key: KeypairSigner,
    pub address: Pubkey,
    pub wallet: WalletId,
}

/// A secp256k1 or EIP-191 Vector account whose secret is in the desk's vault.
pub struct SecpAccount {
    pub address: Pubkey,
    pub wallet: WalletId,
}

/// The online device: a hot wallet that pays, its Vector wallets, the chain.
pub struct Desk {
    pub node: Node,
    pub payer: KeypairSigner,
    pub wallets: Vec<WalletRecord>,
    pub xmss_retries: XmssRetryCache,
    /// What the app's hash-based keys have signed, outside the vault.
    pub leaf_marks: LeafMarks<MemoryStore>,
    /// A vault the wallets are copied into for each press of Sign, so leaves
    /// are reserved as the app reserves them. Made on the first press.
    vault: Option<Vault<MemoryStore>>,
}

impl Desk {
    pub fn new() -> Self {
        let mut chain = Chain::new();
        let payer = KeypairSigner::new_random();
        chain.fund(&payer.pubkey(), 10 * SOL);
        let fees = WalletRecord::from_keypair_seed("Fees", payer.seed_bytes());
        Self {
            node: Node(Mutex::new(chain)),
            payer,
            wallets: vec![fees],
            xmss_retries: XmssRetryCache::default(),
            leaf_marks: LeafMarks::open(MemoryStore::new()),
            vault: None,
        }
    }

    pub fn chain(&self) -> std::sync::MutexGuard<'_, Chain> {
        self.node.chain()
    }

    /// An Ed25519 Vector wallet whose account is initialized and funded. Its
    /// key is kept in its record when `here`, and is on another device otherwise.
    pub fn add_vector(&mut self, label: &str, here: bool) -> Account {
        let key = KeypairSigner::new_random();
        let bytes = key.pubkey().to_bytes().to_vec();
        let address = self.init_vector(SchemeId::Ed25519, &bytes);
        let authority = if here { held(&key) } else { Authority::Outside };
        let id = self.push_vector(label, SchemeId::Ed25519, bytes, authority);
        Account {
            key,
            address,
            wallet: id,
        }
    }

    /// A secp256k1 Vector wallet with its secret kept here.
    pub fn add_secp(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_secp256k1_authority();
        let address = self.init_vector(SchemeId::Secp256k1, &public);
        let id = self.push_vector(
            label,
            SchemeId::Secp256k1,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// An EIP-191 Vector wallet with its secret kept here.
    pub fn add_eip191(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_eip191_authority();
        let address = self.init_vector(SchemeId::Eip191, &public);
        let id = self.push_vector(
            label,
            SchemeId::Eip191,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// An ML-DSA-44 Vector wallet with its secret kept here, fully grown.
    pub fn add_mldsa(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_mldsa44_authority();
        let address = self.init_vector(SchemeId::MlDsa44, &public);
        let id = self.push_vector(
            label,
            SchemeId::MlDsa44,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// A Falcon-512 Vector wallet with its secret kept here.
    pub fn add_falcon(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_falcon512_authority();
        let address = self.init_vector(SchemeId::Falcon512, &public);
        let id = self.push_vector(
            label,
            SchemeId::Falcon512,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// An XMSS Vector wallet with its seed kept here.
    pub fn add_xmss(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_xmss_authority();
        let address = self.init_vector(SchemeId::Xmss, &public);
        let id = self.push_vector(
            label,
            SchemeId::Xmss,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// An XMSS Vector wallet whose keys are made from `phrase`.
    pub fn add_xmss_phrase(&mut self, label: &str, phrase: &str) -> SecpAccount {
        let mut wallet =
            WalletRecord::vector_from_phrase(label, SchemeId::Xmss, PROGRAM_ID, phrase, false)
                .unwrap();
        wallet.kind.set_fee_payer(Some(self.wallets[0].id));
        let key = wallet.kind.vector_settings().unwrap().key.to_vec();
        let address = self.init_vector(SchemeId::Xmss, &key);
        let id = wallet.id;
        self.wallets.push(wallet);
        SecpAccount {
            address,
            wallet: id,
        }
    }

    /// This device is lost: `wallet` is typed in again from `phrase` on a
    /// device that has never signed for it.
    pub fn restore_from_phrase(&mut self, wallet: WalletId, phrase: &str) {
        let at = self.wallets.iter().position(|w| w.id == wallet).unwrap();
        let label = self.wallets[at].label.clone();
        let mut restored =
            WalletRecord::vector_from_phrase(label, SchemeId::Xmss, PROGRAM_ID, phrase, true)
                .unwrap();
        restored.id = wallet;
        restored.kind.set_fee_payer(Some(self.wallets[0].id));
        self.wallets[at] = restored;
        self.leaf_marks = LeafMarks::open(MemoryStore::new());
        self.xmss_retries = XmssRetryCache::default();
    }

    /// What the online device reads of `account` and shows as a QR, taken
    /// in by the device that holds the key.
    pub fn adopt_state(
        &mut self,
        wallet: WalletId,
        account: &Pubkey,
    ) -> wallet_core::vector::Adopted {
        let read =
            VectorAccount::parse(&PROGRAM_ID, &self.chain().account(account).unwrap()).unwrap();
        let created_with = self
            .wallets
            .iter()
            .find(|w| w.id == wallet)
            .and_then(|w| w.kind.vector_settings())
            .unwrap()
            .key
            .to_vec();
        let state = wallet_core::vector::account_state(account, &read, &created_with);
        let vault = self.vault.get_or_insert_with(|| {
            let mut vault = Vault::new(MemoryStore::new());
            vault.create("desk").unwrap();
            vault
        });
        vault.data_mut().unwrap().wallets = self.wallets.clone();
        let adopted =
            wallet_core::vector::adopt_account_state(vault, &mut self.leaf_marks, &state).unwrap();
        self.wallets = vault.data().unwrap().wallets.clone();
        adopted
    }

    /// A Winternitz Vector wallet with its seed and a prepared next key.
    pub fn add_winternitz(&mut self, label: &str) -> SecpAccount {
        let (public, secret) = wallet_core::vector::new_winternitz_authority();
        let address = self.init_vector(SchemeId::Winternitz, &public);
        let id = self.push_vector(
            label,
            SchemeId::Winternitz,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        );
        self.wallets
            .iter_mut()
            .find(|w| w.id == id)
            .unwrap()
            .prepare_vector_next_key()
            .unwrap();
        SecpAccount {
            address,
            wallet: id,
        }
    }

    fn init_vector(&self, scheme: SchemeId, key: &[u8]) -> Pubkey {
        let address = address(&PROGRAM_ID, scheme, key).unwrap();
        let instructions = initialize(&self.payer.pubkey(), scheme, key).unwrap();
        let budget = if scheme == SchemeId::MlDsa44 {
            ComputeBudgetSpec {
                cu_limit: Some(1_400_000),
                ..ComputeBudgetSpec::default()
            }
        } else {
            ComputeBudgetSpec::default()
        };
        let mut chain = self.chain();
        chain
            .send_with(&[&self.payer], instructions, budget)
            .unwrap_or_else(|failure| panic!("initialize: {}", logs(&failure)));
        chain.fund(&address, SOL);
        address
    }

    fn push_vector(
        &mut self,
        label: &str,
        scheme: SchemeId,
        key: Vec<u8>,
        authority: Authority,
    ) -> WalletId {
        let mut wallet = WalletRecord::vector(label, scheme, PROGRAM_ID, key, authority);
        wallet.kind.set_fee_payer(Some(self.wallets[0].id));
        let id = wallet.id;
        self.wallets.push(wallet);
        id
    }

    /// What the site sends, paid by `site_payer`, rebuilt on Sign.
    pub fn rebuild(&self, site_payer: Pubkey, instructions: Vec<Instruction>) -> Adapted {
        let (wire, _) = run(compile_fresh(&self.node, site_payer, instructions)).unwrap();
        self.rebuild_wire(&wire)
    }

    pub fn rebuild_wire(&self, wire: &[u8]) -> Adapted {
        let tx = decode_v1_transaction(wire).unwrap();
        let plan = plan_adaptation(&tx, &self.wallets, None)
            .unwrap_or_else(|err| panic!("{err}"))
            .expect("a wallet without a key is asked to sign");
        let (adapted, _) = run(wallet_flows::adapt::rebuild(&self.node, &tx, &plan))
            .unwrap_or_else(|err| panic!("{err}"));
        adapted
    }

    /// Sign here, online, as the Sign screen does it: the accounts are read,
    /// what each hash-based key signs with is recorded (leaf marks, then the
    /// vault), then the Vector keys sign; after the screen's simulation, a
    /// second press signs as the payer.
    pub fn sign(&mut self, wire: &[u8], known: &[VectorContext]) -> Result<Signed, String> {
        let known = match decode_v1_transaction(wire) {
            Ok(tx) => run(wallet_flows::vector::read_context(&self.node, &tx, known))
                .unwrap_or_else(|_| known.to_vec()),
            Err(_) => known.to_vec(),
        };
        let vault = self.vault.get_or_insert_with(|| {
            let mut vault = Vault::new(MemoryStore::new());
            vault.create("desk").unwrap();
            vault
        });
        vault.data_mut().unwrap().wallets = self.wallets.clone();
        let keys = keys_to_sign_with(
            vault,
            &mut self.leaf_marks,
            &self.xmss_retries,
            wire,
            &[&self.payer],
            &known,
            FloorsFrom::Chain,
        )
        .map_err(|err| err.to_string())?;
        self.wallets = vault.data().unwrap().wallets.clone();
        let mut vector = VectorSigning {
            keys: &keys,
            known: &known,
            cache: Some(&mut self.xmss_retries),
            known_is_current: false,
        };
        super::sign_through(
            Some(&self.node),
            wire,
            &[&self.payer as &dyn WalletSigner],
            &mut vector,
        )
        .map_err(|err| err.to_string())
    }

    /// Sign the move of `wallet`'s XMSS account to its next key as the
    /// device that holds the key does, with no node: from the nonces its own
    /// signatures left.
    pub fn sign_rotation(&mut self, wallet: WalletId) -> wallet_core::protocol::KeyRotation {
        let vault = self.vault.get_or_insert_with(|| {
            let mut vault = Vault::new(MemoryStore::new());
            vault.create("desk").unwrap();
            vault
        });
        vault.data_mut().unwrap().wallets = self.wallets.clone();
        let rotation =
            wallet_core::vector::sign_rotation(vault, &mut self.leaf_marks, wallet).unwrap();
        self.wallets = vault.data().unwrap().wallets.clone();
        rotation
    }

    /// Install `wallet`'s next key once the chain shows its account holding
    /// it, as the Wallets row does.
    pub fn install_rotation(&mut self, wallet: WalletId, account: &Pubkey) {
        let read =
            VectorAccount::parse(&PROGRAM_ID, &self.chain().account(account).unwrap()).unwrap();
        let current = read.current_key().unwrap().to_vec();
        self.wallets
            .iter_mut()
            .find(|w| w.id == wallet)
            .unwrap()
            .install_vector_next_secret(&current)
            .unwrap();
    }

    /// Plant the chain's XMSS `next_leaf`. The vault and the leaf marks
    /// are left as they are: signing takes the chain's floor when it is the
    /// highest.
    pub fn set_xmss_leaf(&mut self, account: &Pubkey, leaf: u32) {
        let key_len = SchemeId::Xmss.key_len();
        let start = wallet_core::vector::HEADER_LEN + key_len;
        self.chain().patch_data(account, |data| {
            data[start..start + 4].copy_from_slice(&leaf.to_le_bytes());
        });
    }

    /// Prepare a rotating wrap: make the next key for the wallet's scheme and
    /// store it. Returns the next public key.
    pub fn prepare_xmss_rotation(&mut self, wallet: WalletId) -> Vec<u8> {
        let record = self
            .wallets
            .iter_mut()
            .find(|w| w.id == wallet)
            .expect("wallet");
        record.prepare_vector_next_key().unwrap();
        record.vector_prepared_next_key().unwrap().to_vec()
    }

    /// Add a signature object another device returned, as the Sign screen
    /// does with what it knows of the accounts.
    pub fn accept(&self, wire: &[u8], object: &[u8], known: &[VectorContext]) -> Vec<u8> {
        let QrObject::Signature(signed) = decode_object(object).unwrap() else {
            panic!("a signature object");
        };
        assert!(matches!(signed, SignedObject::AccountAuthorization { .. }));
        let (wire, _) = accept_signature_in(wire, &signed, &AuthContext { vector: known })
            .unwrap_or_else(|err| panic!("{err}"));
        wire
    }

    pub fn land(&self, wire: &[u8]) -> Landed {
        self.chain()
            .send_wire(wire)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)))
    }
}

/// The key kept in the record of its one account.
pub fn held(key: &KeypairSigner) -> Authority {
    Authority::Held {
        secret: SecretBytes(key.seed_bytes().to_vec()),
        next_leaf: 0,
    }
}

/// What a context note carries of the accounts: no keys.
pub fn note(context: &[VectorContext]) -> Vec<VectorContext> {
    context
        .iter()
        .map(|known| VectorContext {
            key: None,
            ..known.clone()
        })
        .collect()
}

/// Another device, offline, that holds `key` for the account at `address`
/// in a record of its own. It was shown `wire` and the note, and returns the
/// one signature object it made.
pub fn device_signs(wire: &[u8], key: &KeypairSigner, note: &[VectorContext]) -> Vec<u8> {
    let bytes = key.pubkey().to_bytes().to_vec();
    let wallets = [WalletRecord::vector(
        "Mine",
        SchemeId::Ed25519,
        PROGRAM_ID,
        bytes,
        held(key),
    )];
    let keys = local_vector_keys(&wallets);
    let mut vector = VectorSigning {
        keys: &keys,
        known: note,
        cache: None,
        known_is_current: false,
    };
    let signed = run(sign_with(None, wire, &[], &mut vector)).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!((signed.authorizations, signed.signatures), (1, 0));
    let tx = decode_v1_transaction(&signed.wire).unwrap();
    let mut objects = vector_signature_objects(&tx, &AuthContext { vector: note });
    objects.retain(|object| object.pubkey == keys[0].account);
    assert_eq!(objects.len(), 1);
    encode_object(&QrObject::Signature(objects.remove(0).signed)).unwrap()
}
