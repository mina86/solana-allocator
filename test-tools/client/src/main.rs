use std::str::FromStr;

use solana_client::rpc_client::RpcClient;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::program_error::ProgramError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::transaction::Transaction;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::UiTransactionEncoding;

type Result<T = (), E = Error> = core::result::Result<T, E>;

/// `usage: client ( <program-id> | -f <keyfile> )`
fn main() -> Result {
    let program_id = get_program_id()?;
    let keypair = read_keypair()?;
    let client = RpcClient::new("http://127.0.0.1:8899");
    send_and_confirm_instruction(&client, &keypair, Instruction {
        program_id,
        accounts: Vec::new(),
        data: Vec::new(),
    })
    .map_err(Error::from)
}

/// Parses command line arguments to get the program ID.
fn get_program_id() -> Result<Pubkey, Error> {
    let mut argv0: std::borrow::Cow<'static, str> = "client".into();
    (|| {
        let mut args = std::env::args();
        argv0 = args.next()?.into();
        let arg = args.next()?;
        if arg == "-f" {
            args.next().map(|path| {
                solana_sdk::signer::keypair::read_keypair_file(path)
                    .map(|keypair| keypair.pubkey())
                    .map_err(Error::from)
            })
        } else {
            Some(solana_sdk::pubkey::Pubkey::from_str(&arg).map_err(|err| {
                Error::Msg(format!("{argv0}: {arg}: {err}").into())
            }))
        }
    })()
    .ok_or_else(|| {
        Error::Msg(
            format!("usage: {argv0} ( <program-id> | -f <keyfile> )").into(),
        )
    })
    .flatten()
}

/// Reads keypair from a hard-coded location.
fn read_keypair() -> Result<Keypair> {
    let home = std::env::var_os("HOME").unwrap();
    let mut path = std::path::PathBuf::from(home);
    path.push(".config/solana/id.json");
    solana_sdk::signer::keypair::read_keypair_file(path).map_err(Error::from)
}

/// Sends a transaction and logs result.
fn send_and_confirm_instruction(
    client: &RpcClient,
    keypair: &Keypair,
    instruction: Instruction,
) -> Result {
    let blockhash = client.get_latest_blockhash()?;
    eprintln!("Latest blockhash: {blockhash}");

    eprintln!("Sending transaction to {}…", instruction.program_id);

    let instructions = [
        ComputeBudgetInstruction::request_heap_frame(256 * 1024),
        ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
        instruction,
    ];

    let message = Message::new_with_blockhash(
        &instructions,
        Some(&keypair.pubkey()),
        &blockhash,
    );
    let mut tx = Transaction::new_unsigned(message);
    tx.sign(&[&keypair], blockhash);

    let sig = match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => sig,
        Err(err) => {
            panic!("{:#?}", err.kind);
        }
    };
    eprintln!("Signature: {sig}");

    let encoding = UiTransactionEncoding::Binary;
    let resp = client.get_transaction(&sig, encoding)?;
    let (slot, tx) = (resp.slot, resp.transaction);
    eprintln!("Executed in slot: {slot}");

    // Print log messages
    let log_messages = tx
        .meta
        .map(|meta| meta.log_messages)
        .ok_or(Error::Msg("No transaction metadata".into()))?;
    if let OptionSerializer::Some(messages) = log_messages {
        for msg in messages {
            println!("{msg}");
        }
        Ok(())
    } else {
        Err(Error::Msg("No log message".into()))
    }
}

#[derive(derive_more::From, derive_more::Display)]
enum Error {
    Msg(std::borrow::Cow<'static, str>),
    Prog(ProgramError),
    Box(Box<dyn std::error::Error>),
}

impl From<solana_client::client_error::ClientError> for Error {
    fn from(err: solana_client::client_error::ClientError) -> Self {
        Self::Box(Box::new(err))
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, fmtr: &mut core::fmt::Formatter) -> core::fmt::Result {
        core::fmt::Display::fmt(self, fmtr)
    }
}
