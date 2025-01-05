use bitcoin::{Address, Amount};
use bitcoind::bitcoincore_rpc::{json::ListUnspentResultEntry, Auth};
use clap::Parser;
use coinswap::{
    taker::{error::TakerError, SwapParams, Taker, TakerBehavior},
    utill::{parse_proxy_auth, setup_taker_logger, ConnectionType},
    wallet::{Destination, RPCConfig, SendAmount},
};
use log::LevelFilter;
use std::{path::PathBuf, str::FromStr};

/// A simple command line app to operate as coinswap client.
///
/// The app works as regular Bitcoin wallet with added capability to perform coinswaps. The app
/// requires a running Bitcoin Core node with RPC access.
///
/// For more detailed usage information, please refer: [taker-cli demo doc link]
///
/// This is early beta, and there are known and unknown bugs. Please report issues at: https://github.com/citadel-tech/coinswap/issues
#[derive(Parser, Debug)]
#[clap(version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"),
author = option_env ! ("CARGO_PKG_AUTHORS").unwrap_or(""))]
struct Cli {
    /// Optional data directory. Default value : "~/.coinswap/taker"
    #[clap(long, short = 'd')]
    data_directory: Option<PathBuf>,

    /// Bitcoin Core RPC address:port value
    #[clap(
        name = "ADDRESS:PORT",
        long,
        short = 'r',
        default_value = "127.0.0.1:18443"
    )]
    pub rpc: String,

    /// Bitcoin Core RPC authentication string. Ex: username:password
    #[clap(name="USER:PASSWORD",short='a',long, value_parser = parse_proxy_auth, default_value = "user:password")]
    pub auth: (String, String),

    /// Sets the taker wallet's name. If the wallet file already exists, it will load that wallet. Default: taker-wallet
    #[clap(name = "WALLET", long, short = 'w')]
    pub wallet_name: Option<String>,

    /// Sets the verbosity level of debug.log file
    #[clap(long, short = 'v', possible_values = &["off", "error", "warn", "info", "debug", "trace"], default_value = "info")]
    pub verbosity: String,

    /// List of commands for various wallet operations
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
enum Commands {
    // TODO: Design a better structure to display different utxos and balance groups.
    /// Lists all currently spendable utxos
    ListUtxo,
    /// Lists all utxos received in incoming swaps
    ListUtxoSwap,
    /// Lists all HTLC utxos (if any)
    ListUtxoContract,
    /// Get the total spendable wallet balance (sats)
    GetBalance,
    /// Get the total balance received from swaps (sats)
    GetBalanceSwap,
    /// Get the total amount stuck in HTLC contracts (sats)
    GetBalanceContract,
    /// Returns a new address
    GetNewAddress,
    /// Send to an external wallet address.
    SendToAddress {
        /// Recipient's address.
        #[clap(long, short = 't')]
        address: String,
        /// Amount to send in sats
        #[clap(long, short = 'a')]
        amount: u64,
        /// Total fee to be paid in sats
        #[clap(long, short = 'f')]
        fee: u64,
    },
    /// Update the offerbook with current market offers and display them
    FetchOffers,

    // TODO: Also add ListOffers command to just list the current book.
    /// Initiate the coinswap process
    Coinswap {
        /// Sets the maker count to swap with. Swapping with less than 2 makers is allowed to maintain client privacy.
        /// Adding more makers in the swap will incure more swap fees.
        #[clap(long, short = 'm', default_value = "2")]
        makers: usize,
        /// Sets the send amount in sats.
        #[clap(long, short = 'a', default_value = "20000")]
        amount: u64,
        /// Sets how many utxos to swap.
        /// The wallet needs to have at least that many utxos, of greater than or equal to the `amount` value.
        #[clap(long, short = 'u', default_value = "1")]
        utxos: u32,
    },
}

fn main() -> Result<(), TakerError> {
    let args = Cli::parse();
    setup_taker_logger(LevelFilter::from_str(&args.verbosity).unwrap());

    let url = if cfg!(feature = "integration-test") {
        "127.0.0.1:18443".to_owned()
    } else {
        args.rpc
    };

    let rpc_config = RPCConfig {
        url,
        auth: Auth::UserPass(args.auth.0, args.auth.1),
        wallet_name: "random".to_string(), // we can put anything here as it will get updated in the init.
    };

    #[cfg(feature = "tor")]
    let connection_type = if cfg!(feature = "integration-test") {
        ConnectionType::CLEARNET
    } else {
        ConnectionType::TOR
    };

    #[cfg(not(feature = "tor"))]
    let connection_type = ConnectionType::CLEARNET;

    let mut taker = Taker::init(
        args.data_directory.clone(),
        args.wallet_name.clone(),
        Some(rpc_config.clone()),
        TakerBehavior::Normal,
        Some(connection_type),
    )?;

    match args.command {
        Commands::ListUtxo => {
            let utxos: Vec<ListUnspentResultEntry> = taker
                .get_wallet()
                .list_all_utxo_spend_info(None)?
                .iter()
                .map(|(l, _)| l.clone())
                .collect();
            println!("{:#?}", utxos);
        }
        Commands::ListUtxoSwap => {
            let utxos: Vec<ListUnspentResultEntry> = taker
                .get_wallet()
                .list_swap_coin_utxo_spend_info(None)?
                .iter()
                .map(|(l, _)| l.clone())
                .collect();
            println!("{:#?}", utxos);
        }
        Commands::ListUtxoContract => {
            let utxos: Vec<ListUnspentResultEntry> = taker
                .get_wallet()
                .list_live_contract_spend_info(None)?
                .iter()
                .map(|(l, _)| l.clone())
                .collect();
            println!("{:#?}", utxos);
        }
        Commands::GetBalanceContract => {
            let balance = taker.get_wallet().balance_live_contract(None)?;
            println!("{:?}", balance);
        }
        Commands::GetBalanceSwap => {
            let balance = taker.get_wallet().balance_swap_coins(None)?;
            println!("{:?}", balance);
        }
        Commands::GetBalance => {
            let balance = taker.get_wallet().spendable_balance()?;
            println!("{:?}", balance);
        }
        Commands::GetNewAddress => {
            let address = taker.get_wallet_mut().get_next_external_address()?;
            println!("{:?}", address);
        }
        Commands::SendToAddress {
            address,
            amount,
            fee,
        } => {
            // NOTE:
            //
            // Currently, we take `fee` instead of `fee_rate` because we cannot calculate the fee for a
            // transaction that hasn't been created yet when only a `fee_rate` is provided.
            //
            // As a result, the user must supply the fee as a parameter, and the function will return the
            // transaction hex and the calculated `fee_rate`.
            // This allows the user to infer what fee is needed for a successful transaction.
            //
            // This approach will be improved in the future BDK integration.

            let fee = Amount::from_sat(fee);

            let amount = Amount::from_sat(amount);

            let coins_to_spend = taker.get_wallet().coin_select(amount + fee)?;

            let destination =
                Destination::Address(Address::from_str(&address).unwrap().assume_checked());

            let tx = taker.get_wallet_mut().spend_from_wallet(
                fee,
                SendAmount::Amount(amount),
                destination,
                &coins_to_spend,
            )?;

            let txid = taker.get_wallet().send_tx(&tx).unwrap();

            println!("{}", txid);
        }

        Commands::FetchOffers => {
            let offerbook = taker.fetch_offers()?;
            println!("{:#?}", offerbook)
        }
        Commands::Coinswap {
            makers,
            utxos,
            amount,
        } => {
            let swap_params = SwapParams {
                send_amount: Amount::from_sat(amount),
                maker_count: makers,
                tx_count: utxos,
                required_confirms: 1,
            };

            taker.do_coinswap(swap_params)?;
            println!("succesfully completed coinswap!! Check `list-utxo` to see the new coins");
        }
    }

    Ok(())
}
