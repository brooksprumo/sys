use {
    crate::{exchange::*, field_as_string},
    chrono::NaiveDate,
    pickledb::{PickleDb, PickleDbDumpPolicy},
    serde::{Deserialize, Serialize},
    solana_sdk::{
        clock::{Epoch, Slot},
        native_token::lamports_to_sol,
        pubkey::Pubkey,
        signature::Signature,
    },
    std::{
        collections::{HashMap, HashSet},
        fmt, fs,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Io: {0}")]
    Io(#[from] std::io::Error),

    #[error("PickleDb: {0}")]
    PickleDb(#[from] pickledb::error::Error),

    #[error("Account already exists: {0}")]
    AccountAlreadyExists(Pubkey),

    #[error("Account does not exist: {0}")]
    AccountDoesNotExist(Pubkey),

    #[error("Pending transfer with signature does not exist: {0}")]
    PendingTransferDoesNotExist(Signature),

    #[error("Pending deposit with signature does not exist: {0}")]
    PendingDepositDoesNotExist(Signature),

    #[error("Account has insufficient balance: {0}")]
    AccountHasInsufficientBalance(Pubkey),

    #[error("Open order not exist: {0}")]
    OpenOrderDoesNotExist(String),
}

pub type DbResult<T> = std::result::Result<T, DbError>;

pub fn new<P: AsRef<Path>>(db_path: P) -> DbResult<Db> {
    let db_path = db_path.as_ref();
    if !db_path.exists() {
        fs::create_dir_all(db_path)?;
    }

    let db_filename = db_path.join("◎.db");
    let credentials_db_filename = db_path.join("🤐.db");

    let db = if db_filename.exists() {
        PickleDb::load_json(db_filename, PickleDbDumpPolicy::DumpUponRequest)?
    } else {
        PickleDb::new_json(db_filename, PickleDbDumpPolicy::DumpUponRequest)
    };

    let credentials_db = if credentials_db_filename.exists() {
        PickleDb::load_json(credentials_db_filename, PickleDbDumpPolicy::DumpUponRequest)?
    } else {
        PickleDb::new_json(credentials_db_filename, PickleDbDumpPolicy::DumpUponRequest)
    };

    Ok(Db {
        db,
        credentials_db,
        auto_save: true,
    })
}

pub struct Db {
    db: PickleDb,
    credentials_db: PickleDb,
    auto_save: bool,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct PendingDeposit {
    pub signature: Signature, // transaction signature of the deposit
    pub exchange: Exchange,
    pub amount: u64,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct PendingTransfer {
    #[serde(with = "field_as_string")]
    pub signature: Signature, // transaction signature of the transfer

    #[serde(with = "field_as_string")]
    pub from_address: Pubkey,
    #[serde(with = "field_as_string")]
    pub to_address: Pubkey,

    pub lots: Vec<Lot>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct OpenOrder {
    pub exchange: Exchange,
    pub pair: String,
    pub order_id: String,
    pub lots: Vec<Lot>,

    #[serde(with = "field_as_string")]
    pub deposit_address: Pubkey,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum LotAcquistionKind {
    EpochReward {
        epoch: Epoch,
        slot: Slot,
    },
    Transaction {
        slot: Slot,
        #[serde(with = "field_as_string")]
        signature: Signature,
    },
    NotAvailable,
}

impl fmt::Display for LotAcquistionKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LotAcquistionKind::EpochReward { epoch, .. } => write!(f, "epoch {} reward", epoch),
            LotAcquistionKind::Transaction { signature, .. } => write!(f, "{}", signature),
            LotAcquistionKind::NotAvailable => {
                write!(f, "other")
            }
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct LotAcquistion {
    pub when: NaiveDate,
    pub price: f64, // USD per SOL
    pub kind: LotAcquistionKind,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Lot {
    pub lot_number: usize,
    pub acquisition: LotAcquistion,
    pub amount: u64, // lamports
}

impl Lot {
    // Figure the amount of income that the Lot incurred
    pub fn income(&self) -> f64 {
        match self.acquisition.kind {
            LotAcquistionKind::EpochReward { .. } | LotAcquistionKind::NotAvailable => {
                self.acquisition.price * lamports_to_sol(self.amount)
            }
            LotAcquistionKind::Transaction { .. } => 0.,
        }
    }
    // Figure the current cap gain/loss for the Lot
    pub fn cap_gain(&self, current_price: f64) -> f64 {
        (current_price - self.acquisition.price) * lamports_to_sol(self.amount)
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum LotDisposalKind {
    Usd {
        exchange: Exchange,
        pair: String,
        order_id: String,
    },
}

impl fmt::Display for LotDisposalKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LotDisposalKind::Usd {
                exchange,
                pair,
                order_id,
            } => write!(f, "{:?} {}, order {}", exchange, pair, order_id),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct DisposedLot {
    pub lot: Lot,
    pub when: NaiveDate,
    pub price: f64, // USD per SOL
    pub kind: LotDisposalKind,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct TrackedAccount {
    #[serde(with = "field_as_string")]
    pub address: Pubkey,
    pub description: String,
    pub last_update_epoch: Epoch,
    pub last_update_balance: u64,
    pub lots: Vec<Lot>,
}

impl TrackedAccount {
    fn assert_lot_balance(&self) -> u64 {
        let lot_balance: u64 = self.lots.iter().map(|lot| lot.amount).sum();
        assert_eq!(
            lot_balance, self.last_update_balance,
            "Lot balance mismatch: {:?}",
            self
        );
        lot_balance
    }

    pub fn extract_lots(&mut self, db: &mut Db, amount: u64) -> DbResult<Vec<Lot>> {
        if self.last_update_balance < amount {
            return Err(DbError::AccountHasInsufficientBalance(self.address));
        }

        let mut lots = std::mem::take(&mut self.lots);
        lots.sort_by_key(|lot| lot.acquisition.when);

        if !lots.is_empty() {
            // Assume the oldest lot is the rent-reserve. Extract it as the last resort
            let first_lot = lots.remove(0);
            lots.push(first_lot);
        }

        let mut extracted_lots = vec![];
        let mut amount_remaining = amount;
        for mut lot in lots {
            if amount_remaining > 0 {
                if lot.amount <= amount_remaining {
                    amount_remaining -= lot.amount;
                    extracted_lots.push(lot);
                } else {
                    let split_lot = Lot {
                        lot_number: db.next_lot_number(),
                        acquisition: lot.acquisition.clone(),
                        amount: amount_remaining,
                    };
                    lot.amount -= amount_remaining;
                    extracted_lots.push(split_lot);
                    self.lots.push(lot);
                    amount_remaining = 0;
                }
            } else {
                self.lots.push(lot);
            }
        }
        self.lots.sort_by_key(|lot| lot.acquisition.when);
        extracted_lots.sort_by_key(|lot| lot.acquisition.when);
        assert_eq!(
            extracted_lots.iter().map(|el| el.amount).sum::<u64>(),
            amount
        );

        self.last_update_balance -= amount;
        self.assert_lot_balance();
        Ok(extracted_lots)
    }

    fn merge_lots(&mut self, lots: Vec<Lot>) {
        let mut amount = 0;
        for lot in lots {
            amount += lot.amount;
            if let Some(mut existing_lot) = self
                .lots
                .iter_mut()
                .find(|l| l.acquisition == lot.acquisition)
            {
                existing_lot.amount += lot.amount;
            } else {
                self.lots.push(lot);
            }
        }
        self.last_update_balance += amount;
        self.assert_lot_balance();
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct SweepStakeAccount {
    #[serde(with = "field_as_string")]
    pub address: Pubkey,
    pub stake_authority: PathBuf,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct TransitorySweepStake {
    #[serde(with = "field_as_string")]
    pub address: Pubkey,
}

impl Db {
    pub fn set_exchange_credentials(
        &mut self,
        exchange: Exchange,
        exchange_credentials: ExchangeCredentials,
    ) -> DbResult<()> {
        self.clear_exchange_credentials(exchange)?;

        self.credentials_db
            .set(&format!("{:?}", exchange), &exchange_credentials)
            .unwrap();

        Ok(self.credentials_db.dump()?)
    }

    pub fn get_exchange_credentials(&self, exchange: Exchange) -> Option<ExchangeCredentials> {
        self.credentials_db.get(&format!("{:?}", exchange))
    }

    pub fn clear_exchange_credentials(&mut self, exchange: Exchange) -> DbResult<()> {
        if self.get_exchange_credentials(exchange).is_some() {
            self.credentials_db.rem(&format!("{:?}", exchange)).ok();
            self.credentials_db.dump()?;
        }
        Ok(())
    }

    pub fn get_configured_exchanges(&self) -> Vec<(Exchange, ExchangeCredentials)> {
        self.credentials_db
            .get_all()
            .into_iter()
            .filter_map(|key| {
                if let Ok(exchange) = key.parse() {
                    self.get_exchange_credentials(exchange)
                        .map(|exchange_credentials| (exchange, exchange_credentials))
                } else {
                    None
                }
            })
            .collect()
    }

    fn auto_save(&mut self, auto_save: bool) -> DbResult<()> {
        self.auto_save = auto_save;
        self.save()
    }

    fn save(&mut self) -> DbResult<()> {
        if self.auto_save {
            self.db.dump()?;
        }
        Ok(())
    }

    pub fn record_deposit(
        &mut self,
        signature: Signature,
        from_address: Pubkey,
        amount: u64,
        exchange: Exchange,
        deposit_address: Pubkey,
    ) -> DbResult<()> {
        if !self.db.lexists("deposits") {
            self.db.lcreate("deposits")?;
        }

        let deposit = PendingDeposit {
            signature,
            exchange,
            amount,
        };
        self.db.ladd("deposits", &deposit).unwrap();

        self.record_transfer(signature, from_address, Some(amount), deposit_address)
        // `record_transfer` calls `save`...
    }

    fn complete_deposit(&mut self, signature: Signature, success: bool) -> DbResult<()> {
        let mut pending_deposits = self.pending_deposits(None);

        let PendingDeposit { signature, .. } = pending_deposits
            .iter()
            .find(|pd| pd.signature == signature)
            .ok_or(DbError::PendingDepositDoesNotExist(signature))?
            .clone();

        pending_deposits.retain(|pd| pd.signature != signature);
        self.db.set("deposits", &pending_deposits).unwrap();

        self.complete_transfer(signature, success) // `complete_transfer` calls `save`...
    }

    pub fn cancel_deposit(&mut self, signature: Signature) -> DbResult<()> {
        self.complete_deposit(signature, true)
    }

    pub fn confirm_deposit(&mut self, signature: Signature) -> DbResult<()> {
        self.complete_deposit(signature, true)
    }

    pub fn pending_deposits(&self, exchange: Option<Exchange>) -> Vec<PendingDeposit> {
        if !self.db.lexists("deposits") {
            return Vec::default();
        }
        self.db
            .liter("deposits")
            .filter_map(|item_iter| item_iter.get_item::<PendingDeposit>())
            .filter(|pending_deposit| {
                if let Some(exchange) = exchange {
                    pending_deposit.exchange == exchange
                } else {
                    true
                }
            })
            .collect()
    }

    pub fn record_order(
        &mut self,
        deposit_account: TrackedAccount,
        exchange: Exchange,
        pair: String,
        order_id: String,
        lots: Vec<Lot>,
    ) -> DbResult<()> {
        let mut open_orders = self.open_orders(None);
        open_orders.push(OpenOrder {
            exchange,
            pair,
            order_id,
            lots,
            deposit_address: deposit_account.address,
        });
        self.db.set("orders", &open_orders).unwrap();
        self.update_account(deposit_account) // `update_account` calls `save`...
    }

    fn complete_order(
        &mut self,
        order_id: &str,
        filled: Option<(f64 /* USD per SOL */, NaiveDate)>,
    ) -> DbResult<()> {
        let mut open_orders = self.open_orders(None);

        let OpenOrder {
            exchange,
            pair,
            order_id,
            lots,
            deposit_address,
        } = open_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .ok_or_else(|| DbError::OpenOrderDoesNotExist(order_id.to_string()))?
            .clone();

        open_orders.retain(|o| o.order_id != order_id);
        self.db.set("orders", &open_orders).unwrap();

        if let Some((price, when)) = filled {
            let mut disposed_lots = self.disposed_lots();
            for lot in lots {
                disposed_lots.push(DisposedLot {
                    lot,
                    when,
                    price,
                    kind: LotDisposalKind::Usd {
                        exchange,
                        pair: pair.clone(),
                        order_id: order_id.clone(),
                    },
                });
            }
            self.db.set("disposed-lots", &disposed_lots).unwrap();
            self.save()
        } else {
            let mut deposit_account = self
                .get_account(deposit_address)
                .ok_or(DbError::AccountDoesNotExist(deposit_address))?;

            deposit_account.merge_lots(lots);
            self.update_account(deposit_account) // `update_account` calls `save`...
        }
    }

    pub fn cancel_order(&mut self, order_id: &str) -> DbResult<()> {
        self.complete_order(order_id, None)
    }

    pub fn confirm_order(&mut self, order_id: &str, price: f64, when: NaiveDate) -> DbResult<()> {
        self.complete_order(order_id, Some((price, when)))
    }

    pub fn open_orders(&self, exchange: Option<Exchange>) -> Vec<OpenOrder> {
        let orders: Vec<OpenOrder> = self.db.get("orders").unwrap_or_default();
        orders
            .into_iter()
            .filter(|order| {
                if let Some(exchange) = exchange {
                    order.exchange == exchange
                } else {
                    true
                }
            })
            .collect()
    }

    pub fn add_account_no_save(&mut self, account: TrackedAccount) -> DbResult<()> {
        account.assert_lot_balance();

        if !self.db.lexists("accounts") {
            self.db.lcreate("accounts")?;
        }

        if self.get_account(account.address).is_some() {
            Err(DbError::AccountAlreadyExists(account.address))
        } else {
            self.db.ladd("accounts", &account).unwrap();
            Ok(())
        }
    }

    pub fn add_account(&mut self, account: TrackedAccount) -> DbResult<()> {
        self.add_account_no_save(account)?;
        self.save()
    }

    pub fn update_account(&mut self, account: TrackedAccount) -> DbResult<()> {
        account.assert_lot_balance();

        let position = self
            .get_account_position(account.address)
            .ok_or(DbError::AccountDoesNotExist(account.address))?;
        assert!(
            self.db
                .lpop::<TrackedAccount>("accounts", position)
                .is_some(),
            "Cannot update unknown account: {}",
            account.address
        );
        self.db.ladd("accounts", &account).unwrap();
        self.save()
    }

    fn remove_account_no_save(&mut self, address: Pubkey) -> DbResult<()> {
        let position = self
            .get_account_position(address)
            .ok_or(DbError::AccountDoesNotExist(address))?;
        assert!(
            self.db
                .lpop::<TrackedAccount>("accounts", position)
                .is_some(),
            "Cannot remove unknown account: {}",
            address
        );
        Ok(())
    }

    pub fn remove_account(&mut self, address: Pubkey) -> DbResult<()> {
        self.remove_account_no_save(address)?;
        self.save()
    }

    fn get_account_position(&self, address: Pubkey) -> Option<usize> {
        if self.db.lexists("accounts") {
            for (position, value) in self.db.liter("accounts").enumerate() {
                if let Some(tracked_account) = value.get_item::<TrackedAccount>() {
                    if tracked_account.address == address {
                        return Some(position);
                    }
                }
            }
        }
        None
    }

    pub fn get_account(&self, address: Pubkey) -> Option<TrackedAccount> {
        if !self.db.lexists("accounts") {
            None
        } else {
            self.db
                .liter("accounts")
                .filter_map(|item_iter| item_iter.get_item::<TrackedAccount>())
                .find(|tracked_account| tracked_account.address == address)
        }
    }

    pub fn get_accounts(&self) -> HashMap<Pubkey, TrackedAccount> {
        if !self.db.lexists("accounts") {
            return HashMap::default();
        }
        self.db
            .liter("accounts")
            .filter_map(|item_iter| {
                item_iter
                    .get_item::<TrackedAccount>()
                    .map(|ta| (ta.address, ta))
            })
            .collect()
    }

    // The caller must call `save()`...
    pub fn next_lot_number(&mut self) -> usize {
        let lot_number = self.db.get::<usize>("next_lot_number").unwrap_or(0);
        self.db.set("next_lot_number", &(lot_number + 1)).unwrap();
        lot_number
    }

    pub fn get_sweep_stake_account(&self) -> Option<SweepStakeAccount> {
        self.db.get("sweep-stake-account")
    }

    pub fn set_sweep_stake_account(
        &mut self,
        sweep_stake_account: SweepStakeAccount,
    ) -> DbResult<()> {
        let _ = self
            .get_account_position(sweep_stake_account.address)
            .ok_or(DbError::AccountDoesNotExist(sweep_stake_account.address))?;
        self.db
            .set("sweep-stake-account", &sweep_stake_account)
            .unwrap();
        self.save()
    }

    pub fn get_transitory_sweep_stake_addresses(&self) -> HashSet<Pubkey> {
        self.db
            .get::<Vec<TransitorySweepStake>>("transitory-sweep-stake-accounts")
            .unwrap_or_default()
            .into_iter()
            .map(|tss| tss.address)
            .collect()
    }

    pub fn add_transitory_sweep_stake_address(
        &mut self,
        address: Pubkey,
        current_epoch: Epoch,
    ) -> DbResult<()> {
        let mut transitory_sweep_stake_addresses = self.get_transitory_sweep_stake_addresses();

        if transitory_sweep_stake_addresses.contains(&address) {
            Err(DbError::AccountAlreadyExists(address))
        } else {
            transitory_sweep_stake_addresses.insert(address);
            self.set_transitory_sweep_stake_addresses(transitory_sweep_stake_addresses)
        }?;

        self.add_account_no_save(TrackedAccount {
            address,
            description: "Transitory stake account".to_string(),
            last_update_balance: 0,
            last_update_epoch: current_epoch,
            lots: vec![],
        })
    }

    pub fn remove_transitory_sweep_stake_address(&mut self, address: Pubkey) -> DbResult<()> {
        let _ = self.remove_account_no_save(address);

        let mut transitory_sweep_stake_addresses = self.get_transitory_sweep_stake_addresses();

        if !transitory_sweep_stake_addresses.contains(&address) {
            Err(DbError::AccountDoesNotExist(address))
        } else {
            transitory_sweep_stake_addresses.remove(&address);
            self.set_transitory_sweep_stake_addresses(transitory_sweep_stake_addresses)
        }
    }

    fn set_transitory_sweep_stake_addresses<T>(
        &mut self,
        transitory_sweep_stake_addresses: T,
    ) -> DbResult<()>
    where
        T: IntoIterator<Item = Pubkey>,
    {
        self.db
            .set(
                "transitory-sweep-stake-accounts",
                &transitory_sweep_stake_addresses
                    .into_iter()
                    .map(|address| TransitorySweepStake { address })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        self.save()
    }

    pub fn record_transfer(
        &mut self,
        signature: Signature,
        from_address: Pubkey,
        amount: Option<u64>, // None = all
        to_address: Pubkey,
    ) -> DbResult<()> {
        let mut pending_transfers = self.pending_transfers();

        let mut from_account = self
            .get_account(from_address)
            .ok_or(DbError::AccountDoesNotExist(from_address))?;
        let _to_account = self
            .get_account(to_address)
            .ok_or(DbError::AccountDoesNotExist(to_address))?;

        pending_transfers.push(PendingTransfer {
            signature,
            from_address,
            to_address,
            lots: from_account
                .extract_lots(self, amount.unwrap_or(from_account.last_update_balance))?,
        });

        self.db.set("transfers", &pending_transfers).unwrap();
        self.update_account(from_account) // `update_account` calls `save`...
    }

    fn complete_transfer(&mut self, signature: Signature, success: bool) -> DbResult<()> {
        let mut pending_transfers = self.pending_transfers();

        let PendingTransfer {
            signature,
            from_address,
            to_address,
            lots,
        } = pending_transfers
            .iter()
            .find(|pt| pt.signature == signature)
            .ok_or(DbError::PendingTransferDoesNotExist(signature))?
            .clone();

        pending_transfers.retain(|pt| pt.signature != signature);
        self.db.set("transfers", &pending_transfers).unwrap();

        let mut from_account = self
            .get_account(from_address)
            .ok_or(DbError::AccountDoesNotExist(from_address))?;
        let mut to_account = self
            .get_account(to_address)
            .ok_or(DbError::AccountDoesNotExist(to_address))?;

        if success {
            to_account.merge_lots(lots);
        } else {
            from_account.merge_lots(lots);
        }

        self.auto_save(false)?;
        self.update_account(to_account)?;
        self.update_account(from_account)?;
        self.auto_save(true)
    }

    pub fn cancel_transfer(&mut self, signature: Signature) -> DbResult<()> {
        self.complete_transfer(signature, false)
    }

    pub fn confirm_transfer(&mut self, signature: Signature) -> DbResult<()> {
        self.complete_transfer(signature, true)
    }

    pub fn pending_transfers(&self) -> Vec<PendingTransfer> {
        self.db.get("transfers").unwrap_or_default()
    }

    pub fn disposed_lots(&self) -> Vec<DisposedLot> {
        self.db.get("disposed-lots").unwrap_or_default()
    }
}
