//! Program entrypoint definition for the Solana runtime.

use solana_program::account_info::AccountInfo;
use solana_program::entrypoint::ProgramResult;
use solana_program::pubkey::Pubkey;

use crate::processor::Processor;

#[cfg(all(target_os = "solana", not(feature = "no-entrypoint")))]
solana_program::entrypoint!(process_instruction);

/// Global program entrypoint invoked by the Solana runtime.
///
/// # Errors
///
/// Returns [`ProgramResult`] representing the outcome of instruction processing.
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    Processor::process(program_id, accounts, instruction_data)
}
