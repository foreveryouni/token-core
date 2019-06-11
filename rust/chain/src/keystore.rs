use bitcoin::network::constants::Network;
use bitcoin::util::address::Address;
use secp256k1::Secp256k1;
use bitcoin::PrivateKey;
use bitcoin::util::bip32::{ExtendedPrivKey, ExtendedPubKey, DerivationPath};
use bip39::{Mnemonic, Language};
use std::str::FromStr;
use bitcoin_hashes::hex::{ToHex, FromHex};
use serde::{Deserialize, Serialize};
use tcx_crypto::{Crypto, Pbkdf2Params, EncPair, TokenError};
use uuid::Uuid;

#[derive(Debug, Clone)]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Source {
    Wif,
    Private,
    Keystore,
    Mnemonic,
    NewIdentity,
    RecoveredIdentity
}

#[derive(Debug, Clone)]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub name: String,
    pub password_hint: String,
    pub chain_type: String,
    pub timestamp: i64,
    pub network: String,
    pub source: Source,
    pub mode: String,
    pub wallet_type: String,
    pub seg_wit: String,
}

impl Default for Metadata {
    fn default() -> Self {
        Metadata {
            name: String::from("BCH"),
            password_hint: String::new(),
            chain_type: String::from("BCH"),
            timestamp: 0,
            network: String::from("MAINNET"),
            source: Source::Mnemonic,
            mode: String::from("NORMAL"),
            wallet_type: String::from("HD"),
            seg_wit: String::from("NONE"),
        }
    }
}



pub trait Keystore {
    fn get_metadata(&self) -> Metadata;
    fn get_address(&self) -> String;
    fn decrypt_cipher_text(&self, password: &str) -> Result<Vec<u8>, TokenError> ;
}

pub struct V3Keystore {
    pub id: String,
    pub version: i32,
    pub address: String,
    pub crypto: Crypto<Pbkdf2Params>,
    pub metadata: Metadata
}

impl V3Keystore {
    pub fn new(password: &str, prv_key: &str) -> Result<V3Keystore, TokenError> {
        let crypto : Crypto<Pbkdf2Params> = Crypto::new(password, prv_key.to_owned().as_bytes());
        let mut metadata = Metadata::default();
        metadata.source = Source::Wif;
        let keystore = V3Keystore {
            id: Uuid::new_v4().to_hyphenated().to_string(),
            version: 3,
            address: generate_address_from_wif(prv_key),
            crypto,
            metadata
        };
        Ok(keystore)
    }
}

impl Keystore for V3Keystore {
    fn get_metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn get_address(&self) -> String {
        self.address.clone()
    }

    fn decrypt_cipher_text(&self, password: &str) -> Result<Vec<u8>, TokenError> {
        self.crypto.decrypt(password)
    }
}

pub struct V3MnemonicKeystore {
    id: String,
    version: i32,
    address: String,
    crypto: Crypto<Pbkdf2Params>,
    mnemonic_path: String,
    enc_mnemonic: EncPair,
}

impl V3MnemonicKeystore {
    pub fn new(password: &str, mnemonic: &str, path: &str) -> Result<V3MnemonicKeystore, TokenError> {
        let prv_key = Self::generate_prv_key_from_mnemonic(mnemonic, path)?;
        let crypto : Crypto<Pbkdf2Params> = Crypto::new(password, &prv_key.to_bytes());
        let enc_mnemonic = crypto.derive_enc_pair(password, mnemonic.as_bytes());

        let keystore = V3MnemonicKeystore {
            id: Uuid::new_v4().to_hyphenated().to_string(),
            version: 3,
            address: Self::address_from_private_key(&prv_key),
            crypto,
            mnemonic_path: String::from(path),
            enc_mnemonic
        };
        return Ok(keystore);

    }

    fn generate_prv_key_from_mnemonic(mnemonic_str: &str, path: &str) -> Result<PrivateKey, TokenError> {
         if let Ok(mnemonic) = Mnemonic::from_phrase(mnemonic_str, Language::English) {
             let seed = bip39::Seed::new(&mnemonic, &"");
             println!("hex: {}", seed.to_hex());
             let s = Secp256k1::new();
             let sk = ExtendedPrivKey::new_master(Network::Bitcoin, seed.as_bytes()).unwrap();

             let path = DerivationPath::from_str(path).unwrap();
             let main_address_pk = sk.derive_priv(&s, &path).unwrap();
             return Ok(main_address_pk.private_key);
         } else {
             return Err(TokenError::from("invalid_mnemonic"));
         }
    }

    fn address_from_private_key(pk: &PrivateKey) -> String {
        let s = Secp256k1::new();
        let pub_key = pk.public_key(&s);
        // Generate pay-to-pubkey-hash address
        let address = Address::p2pkh(&pub_key, Network::Bitcoin);
        return address.to_string();
    }

    pub fn export_private_key(&self, password: &str) -> Result<String, TokenError> {
        let pk_bytes = self.crypto.decrypt(password)?;
        let pk = pk_bytes.to_hex();
        return Ok(pk);
    }
}

fn generate_address_from_wif(wif : &str) -> String {
    let s = Secp256k1::new();
    let prv_key = PrivateKey::from_wif(wif).unwrap();
    let pub_key = prv_key.public_key(&s);
    // Generate pay-to-pubkey-hash address
    let address = Address::p2pkh(&pub_key, Network::Bitcoin);

    println!("{}", address.to_string());
    return address.to_string();
}



#[cfg(test)]
mod tests {
    use super::*;

    static PASSWORD: &'static str = "Insecure Pa55w0rd";
    static MNEMONIC: &'static str = "inject kidney empty canal shadow pact comfort wife crush horse wife sketch";
    static ETHEREUM_PATH: &'static str = "m/44'/60'/0'/0/0";



    #[test]
    pub fn new_v3_mnemonic_keystore() {
        let keystore = V3MnemonicKeystore::new(&PASSWORD, &MNEMONIC, &ETHEREUM_PATH);

        assert!(keystore.is_ok());

        let keystore = keystore.unwrap();
        assert_eq!("16Hp1Ga779iaTe1TxUFDEBqNCGvfh3EHDZ", keystore.address);

//        println!(se)
    }

    #[test]
    pub fn bch_address() {
        let address = generate_address_from_wif("L1uyy5qTuGrVXrmrsvHWHgVzW9kKdrp27wBC7Vs6nZDTF2BRUVwy");
        assert_eq!("17XBj6iFEsf8kzDMGQk5ghZipxX49VXuaV", address);

    }


}

