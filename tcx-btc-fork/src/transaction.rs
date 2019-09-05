use tcx_chain::{HdKeystore, Secp256k1Curve, TransactionSigner, TxSignResult};

use bitcoin::network::constants::Network;
use bitcoin::{Address as BtcAddress, OutPoint, Script, Transaction, TxIn, TxOut};
use bitcoin_hashes::hex::FromHex;
use bitcoin_hashes::sha256d::Hash as Hash256;
use bitcoin_hashes::{sha256d, Hash};
use tcx_chain::Transaction as TraitTransaction;

use crate::bip143_with_forkid::SighashComponentsWithForkId;
use crate::Result;
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::serialize;
use bitcoin_hashes::hex::ToHex;
use serde::{Deserialize, Deserializer, Serialize};
use std::str::FromStr;

use crate::address::{network_from_coin, BtcForkAddress};
use tcx_chain::curve::{PrivateKey, Secp256k1PublicKey};

use crate::Error;
use crate::ExtendedPubKeyExtra;
use bitcoin::util::base58::from;
use bitcoin::util::bip32::ExtendedPubKey;
use bitcoin_hashes::hash160;
use tcx_chain::bips::get_account_path;
use tcx_chain::curve::PublicKey;

const DUST: u64 = 546;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utxo {
    pub tx_hash: String,
    pub vout: i32,
    #[serde(with = "string")]
    pub amount: i64,
    pub address: String,
    pub script_pub_key: String,
    pub derived_path: String,
    #[serde(default)]
    pub sequence: i64,
}

mod string {
    use std::fmt::Display;
    use std::str::FromStr;

    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Display,
        S: Serializer,
    {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

pub struct BitcoinForkTransaction {
    pub to: String,
    pub amount: i64,
    pub unspents: Vec<Utxo>,
    pub memo: String,
    pub fee: i64,
    pub change_idx: u32,
    pub coin: String,
    pub is_seg_wit: bool,
}

impl TraitTransaction for BitcoinForkTransaction {}

impl TransactionSigner<BitcoinForkTransaction, TxSignResult> for HdKeystore {
    fn sign_transaction(
        &self,
        tx: &BitcoinForkTransaction,
        password: Option<&str>,
    ) -> Result<TxSignResult> {
        let account = self
            .account(tx.coin.to_uppercase().as_str())
            .ok_or(format_err!("account_not_found"))?;
        let path = &account.derivation_path;
        let extra = ExtendedPubKeyExtra::from(account.extra.clone());

        let paths = tx.collect_prv_keys_paths(path)?;

        tcx_ensure!(password.is_some(), tcx_crypto::Error::InvalidPassword);
        let priv_keys =
            &self.key_at_paths(tx.coin.to_uppercase().as_str(), &paths, password.unwrap())?;

        let xpub = extra.xpub()?;
        let change_addr = tx.change_address(&xpub)?;
        tx.sign_transaction(&priv_keys, &change_addr)
    }
}

impl BitcoinForkTransaction {
    fn collect_prv_keys_paths(&self, path: &str) -> Result<Vec<String>> {
        let mut paths: Vec<String> = vec![];
        let account_path = get_account_path(path)?;

        for unspent in &self.unspents {
            let derived_path = unspent.derived_path.trim();
            let path_with_space = derived_path.replace("/", " ");

            let path_idxs: Vec<&str> = path_with_space.split(" ").collect();
            ensure!(path_idxs.len() == 2, "derived path must be x/x");

            paths.push(format!("{}/{}", account_path, derived_path));
        }
        Ok(paths)
    }

    fn receive_script_pubkey(&self) -> Result<Script> {
        let addr = BtcForkAddress::from_str(&self.to)?;
        Ok(addr.script_pubkey())
    }

    fn sign_hash_and_pub_key(
        &self,
        pri_key: &impl PrivateKey,
        hash: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let signature_bytes = pri_key.sign(&hash)?;
        let fork_id = self.fork_id()?;
        let raw_bytes: Vec<u8> = vec![0x01 | fork_id];
        let sig_bytes: Vec<u8> = [signature_bytes, raw_bytes].concat();
        let pub_key_bytes = pri_key.public_key().to_bytes();
        Ok((sig_bytes, pub_key_bytes))
    }

    fn change_address(&self, xpub: &str) -> Result<BtcForkAddress> {
        let from = BtcForkAddress::convert_to_legacy_if_need(
            &self.unspents.first().expect("first_utxo").address,
        )?;
        let change_path = format!("0/{}", &self.change_idx);
        let pub_key = Secp256k1Curve::derive_pub_key_at_path(&xpub, &change_path)?;
        BtcForkAddress::address_like(&from, &pub_key)
    }

    fn tx_outs(&self, change_script_pubkey: Script) -> Result<Vec<TxOut>> {
        let mut total_amount = 0;

        for unspent in &self.unspents {
            total_amount += unspent.amount;
        }

        ensure!(
            total_amount >= (self.amount + self.fee),
            "total amount must ge amount + fee"
        );

        let mut tx_outs: Vec<TxOut> = vec![];

        let receive_script_pubkey = self.receive_script_pubkey()?;
        let receiver_tx_out = TxOut {
            value: self.amount as u64,
            script_pubkey: receive_script_pubkey,
        };
        tx_outs.push(receiver_tx_out);
        let change_amount = total_amount - self.amount - self.fee;

        if change_amount > DUST as i64 {
            let change_tx_out = TxOut {
                value: change_amount as u64,
                script_pubkey: change_script_pubkey,
            };
            tx_outs.push(change_tx_out);
        }
        Ok(tx_outs)
    }

    fn tx_inputs(&self) -> Vec<TxIn> {
        let mut tx_inputs: Vec<TxIn> = vec![];

        for unspent in &self.unspents {
            tx_inputs.push(TxIn {
                previous_output: OutPoint {
                    txid: Hash256::from_hex(&unspent.tx_hash).unwrap(),
                    vout: unspent.vout as u32,
                },
                script_sig: Script::new(),
                sequence: 0xFFFFFFFF,
                witness: vec![],
            });
        }
        tx_inputs
    }

    fn fork_id(&self) -> Result<u8> {
        let network = network_from_coin(&self.coin).ok_or(Error::UnsupportedChain)?;
        Ok(network.fork_id)
    }

    fn script_sigs_sign(
        &self,
        tx: &Transaction,
        prv_keys: &[impl PrivateKey],
    ) -> Result<Vec<Script>> {
        let mut script_sigs: Vec<Script> = vec![];
        for i in 0..tx.input.len() {
            let tx_in = &tx.input[i];
            let unspent = &self.unspents[i];
            let pub_key = prv_keys[i].public_key();
            let fork_id = self.fork_id()?;

            let network = network_from_coin(&self.coin).ok_or(Error::UnsupportedChain)?;
            let from_addr = BtcForkAddress::p2pkh(&pub_key, &network)?;
            let script = from_addr.script_pubkey();
            let hash = tx.signature_hash(i, &script, 0x01 | fork_id as u32);
            let prv_key = &prv_keys[i];
            let script_sig_and_pub_key = self.sign_hash_and_pub_key(prv_key, &hash.into_inner())?;
            let script = Builder::new()
                .push_slice(&script_sig_and_pub_key.0)
                .push_slice(&script_sig_and_pub_key.1)
                .into_script();
            script_sigs.push(script);
        }
        Ok(script_sigs)
    }

    fn witness_sign(
        &self,
        tx: &Transaction,
        shc: &SighashComponentsWithForkId,
        prv_keys: &[impl PrivateKey],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut witnesses: Vec<(Vec<u8>, Vec<u8>)> = vec![];
        for i in 0..tx.input.len() {
            let tx_in = &tx.input[i];
            let unspent = &self.unspents[i];
            let pub_key = prv_keys[i].public_key();
            let fork_id = self.fork_id()?;
            let pub_key_hash = hash160::Hash::hash(&pub_key.to_bytes()).into_inner();
            let script_hex = format!("76a914{}88ac", hex::encode(pub_key_hash));
            let script = Script::from(hex::decode(script_hex)?);
            let hash =
                shc.sighash_all(tx_in, &script, unspent.amount as u64, 0x01 | fork_id as u32);

            let prv_key = &prv_keys[i];
            witnesses.push((self.sign_hash_and_pub_key(prv_key, &hash.into_inner())?));
        }
        Ok(witnesses)
    }

    fn sign_transaction(
        &self,
        prv_keys: &[impl PrivateKey],
        change_addr: &BtcForkAddress,
    ) -> Result<TxSignResult> {
        let change_script_pubkey = change_addr.script_pubkey();
        let tx_outs = self.tx_outs(change_script_pubkey)?;
        let tx_inputs = self.tx_inputs();
        let version = if self.is_seg_wit { 2 } else { 1 };
        let tx = Transaction {
            version,
            lock_time: 0,
            input: tx_inputs,
            output: tx_outs,
        };

        let input_with_sigs: Vec<TxIn>;
        if self.is_seg_wit {
            let sig_hash_components = SighashComponentsWithForkId::new(&tx);
            let witnesses: Vec<(Vec<u8>, Vec<u8>)> =
                self.witness_sign(&tx, &sig_hash_components, &prv_keys)?;
            input_with_sigs = tx
                .input
                .iter()
                .enumerate()
                .map(|(i, txin)| {
                    let pub_key = prv_keys[i].public_key();
                    let hash = hash160::Hash::hash(&pub_key.to_bytes()).into_inner();
                    let hex = format!("160014{}", hex::encode(&hash));

                    TxIn {
                        script_sig: Script::from(hex::decode(hex).unwrap()),
                        witness: vec![witnesses[i].0.clone(), witnesses[i].1.clone()],
                        ..*txin
                    }
                })
                .collect();
        } else {
            let sign_scripts = self.script_sigs_sign(&tx, &prv_keys)?;
            input_with_sigs = tx
                .input
                .iter()
                .enumerate()
                .map(|(i, txin)| TxIn {
                    script_sig: sign_scripts[i].clone(),
                    witness: vec![],
                    ..*txin
                })
                .collect();
        }
        let signed_tx = Transaction {
            version: tx.version,
            lock_time: tx.lock_time,
            input: input_with_sigs,
            output: tx.output.clone(),
        };

        let tx_bytes = serialize(&signed_tx);

        Ok(TxSignResult {
            signature: tx_bytes.to_hex(),
            tx_hash: signed_tx.txid().into_inner().to_hex(),
            wtx_id: "".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExtendedPubKeyExtra;
    use secp256k1::SecretKey;
    use tcx_chain::curve::CurveType;
    use tcx_chain::keystore::CoinInfo;
    use tcx_chain::{HdKeystore, Metadata, Secp256k1PrivateKey};

    static PASSWORD: &'static str = "Insecure Pa55w0rd";
    static MNEMONIC: &'static str =
        "inject kidney empty canal shadow pact comfort wife crush horse wife sketch";
    static BCH_MAIN_PATH: &'static str = "m/44'/145'/0'";

    //
    #[test]
    pub fn bch_signer() {
        let meta = Metadata::default();
        let mut keystore = HdKeystore::from_mnemonic(&MNEMONIC, &PASSWORD, meta);

        let coin_info = CoinInfo {
            symbol: "BCH".to_string(),
            derivation_path: "m/44'/145'/0'/0/0".to_string(),
            curve: CurveType::SECP256k1,
        };
        let _ = keystore.derive_coin::<BtcForkAddress, ExtendedPubKeyExtra>(&coin_info, &PASSWORD);
        let unspents = vec![Utxo {
            tx_hash: "115e8f72f39fad874cfab0deed11a80f24f967a84079fb56ddf53ea02e308986".to_string(),
            vout: 0,
            amount: 50000,
            address: "17XBj6iFEsf8kzDMGQk5ghZipxX49VXuaV".to_string(),
            script_pub_key: "76a91447862fe165e6121af80d5dde1ecb478ed170565b88ac".to_string(),
            derived_path: "0/1".to_string(),
            sequence: 0,
        }];
        let tran = BitcoinForkTransaction {
            to: "1Gokm82v6DmtwKEB8AiVhm82hyFSsEvBDK".to_string(),
            amount: 15000,
            unspents,
            memo: "".to_string(),
            fee: 35000,
            change_idx: 0,
            coin: "BCH".to_string(),
            is_seg_wit: false,
        };

        let sign_ret = keystore.sign_transaction(&tran, Some(&PASSWORD)).unwrap();
        // todo: not a real test data, it's works at WIF: L1uyy5qTuGrVXrmrsvHWHgVzW9kKdrp27wBC7Vs6nZDTF2BRUVwy
        assert_eq!(sign_ret.signature, "01000000018689302ea03ef5dd56fb7940a867f9240fa811eddeb0fa4c87ad9ff3728f5e11000000006b483045022100be283eb3c936fbdc9159d7067cf3bf44b40c5fc790e6f06368c404a6c1962ebb022071741ed6e1d034f300d177582c870934d4b155d0eb40e6eda99b3e95323a4666412102cc987e200a13c771d9c840cd08db93debf4d4443cec3e084a4cde2aad4cfa77dffffffff01983a0000000000001976a914ad618cf4333b3b248f9744e8e81db2964d0ae39788ac00000000");
    }

    #[test]
    fn test_sign_ltc() {
        let unspents = vec![Utxo {
            tx_hash: "a477af6b2667c29670467e4e0728b685ee07b240235771862318e29ddbe58458".to_string(),
            vout: 0,
            amount: 1000000,
            address: "mszYqVnqKoQx4jcTdJXxwKAissE3Jbrrc1".to_string(),
            script_pub_key: "76a91488d9931ea73d60eaf7e5671efc0552b912911f2a88ac".to_string(),
            derived_path: "0/0".to_string(),
            sequence: 0,
        }];
        let tran = BitcoinForkTransaction {
            to: "mrU9pEmAx26HcbKVrABvgL7AwA5fjNFoDc".to_string(),
            amount: 500000,
            unspents,
            memo: "".to_string(),
            fee: 100000,
            change_idx: 1,
            coin: "LTC-TESTNET".to_string(),
            is_seg_wit: false,
        };

        let prv_key =
            Secp256k1PrivateKey::from_wif("cSBnVM4xvxarwGQuAfQFwqDg9k5tErHUHzgWsEfD4zdwUasvqRVY")
                .unwrap();
        let change_addr = BtcForkAddress::from_str("mgBCJAsvzgT2qNNeXsoECg2uPKrUsZ76up").unwrap();
        //        let sign_ret = keystore.sign_transaction(&tran, Some(&PASSWORD)).unwrap();
        let expected = tran.sign_transaction(&vec![prv_key], &change_addr).unwrap();
        assert_eq!(expected.signature, "01000000015884e5db9de218238671572340b207ee85b628074e7e467096c267266baf77a4000000006a473044022029063983b2537e4aa15ee838874269a6ba6f5280297f92deb5cd56d2b2db7e8202207e1581f73024a48fce1100ed36a1a48f6783026736de39a4dd40a1ccc75f651101210223078d2942df62c45621d209fab84ea9a7a23346201b7727b9b45a29c4e76f5effffffff0220a10700000000001976a9147821c0a3768aa9d1a37e16cf76002aef5373f1a888ac801a0600000000001976a914073b7eae2823efa349e3b9155b8a735526463a0f88ac00000000");
    }

    #[test]
    fn test_sign_segwit_ltc() {
        let unspents = vec![Utxo {
            tx_hash: "e868b66e75376add2154acb558cf45ff7b723f255e2aca794da1548eb945ba8b".to_string(),
            vout: 1,
            amount: 19850000,
            address: "MV3hqxhhcGxCdeLXpZKRCabtUApRXixgid".to_string(),
            script_pub_key: "76a91488d9931ea73d60eaf7e5671efc0552b912911f2a88ac".to_string(),
            derived_path: "1/0".to_string(),
            sequence: 0,
        }];
        let tran = BitcoinForkTransaction {
            to: "M7xo1Mi1gULZSwgvu7VVEvrwMRqngmFkVd".to_string(),
            amount: 19800000,
            unspents,
            memo: "".to_string(),
            fee: 50000,
            change_idx: 1,
            coin: "LTC".to_string(),
            is_seg_wit: true,
        };
        //
        let prv_key = Secp256k1PrivateKey {
            compressed: true,
            network: Network::Bitcoin,
            key: SecretKey::from_slice(
                &hex::decode("f3731f49d830c109e054522df01a9378383814af5b01a9cd150511f12db39e6e")
                    .unwrap(),
            )
            .unwrap(),
        };
        let change_addr = BtcForkAddress::from_str("MV3hqxhhcGxCdeLXpZKRCabtUApRXixgid").unwrap();
        let expected = tran.sign_transaction(&vec![prv_key], &change_addr).unwrap();
        assert_eq!(expected.signature, "020000000001018bba45b98e54a14d79ca2a5e253f727bff45cf58b5ac5421dd6a37756eb668e801000000171600147b03478d2f7c984179084baa38f790ed1d37629bffffffff01c01f2e010000000017a91400aff21f24bc08af58e41e4186d8492a10b84f9e8702483045022100d0cc3d94c7b7b34fdcc2adc4fd3f735560407581afd6caa11c8d04b963a048a00220777d98e0122fe97206875f49556a401dfc449739ec30e44cb9ed9b92a0b3ff1b01210209c629c64829ec2e99703600ee86c7161a9ed13213e714726210274c29cf780900000000");
    }
}
