use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::providers::ProviderBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::icp::IcpSigner;
use alloy::signers::Signer;
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::icp::IcpConfig;
use alloy::{primitives::Address, sol};
use candid::{CandidType, Deserialize, Nat, Principal};
use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpHeader, HttpMethod, HttpResponse, TransformArgs,
    TransformContext,
};
use ic_cdk_macros::*;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use std::{cell::RefCell, str::FromStr};
use types::{
    EthCallParams, GenericDepositDetail, JsonRpcRequest, JsonRpcResult, NetworkDetails, Networks,
    UserDeposit, UserVaultDeposit,
};
use util::{create_icp_signer, from_hex, generate_rpc_service, to_hex, transform_http_request};

mod types;
mod util;

const HTTP_CYCLES: u128 = 100_000_000; // Default cycles for HTTP request
const MAX_RESPONSE_BYTES: u64 = 2048; // Default response size limit
const NULL_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

sol! {
    #[derive(Debug, Serialize, Deserialize)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function transfer(address recipient, uint256 amount) external returns (bool);

    }
}

sol! {
    #[allow(missing_docs, clippy::too_many_arguments)]
    #[sol(rpc)]
    ERC20Token,
    "../../abi/ERC20.json"
}

sol! {
    #[allow(missing_docs, clippy::too_many_arguments)]
    #[sol(rpc)]
    InsurancePool,
    "../../abi/InsurancePool.json"
}

sol! {
    #[allow(missing_docs, clippy::too_many_arguments)]
    #[sol(rpc)]
    VaultContract,
    "../../abi/Vault.json"
}

sol! {
    #[derive(Debug, Serialize, Deserialize)]
    struct GenericDepositDetails {
        address lp;
        uint256 amount;
        uint256 poolId;
        uint256 dailyPayout;
        Status status;
        uint256 daysLeft;
        uint256 startDate;
        uint256 expiryDate;
        uint256 accruedPayout;
        DepositType pdt;
        AssetDepositType adt;
        address asset; // Vault deposit or normal pool deposit?
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    enum Status {
        Active,
        Due,
        Withdrawn
    }

    #[derive(Debug, Serialize, Deserialize)]
    enum AssetDepositType {
        Native,
        ERC20
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    enum DepositType {
        Normal,
        Vault
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    enum ProposalStaus {
        Submitted,
        Pending,
        Approved,
        Claimed,
        Rejected
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct VaultDeposit {
        address lp;
        uint256 amount;
        uint256 vaultId;
        uint256 dailyPayout;
        Status status;
        uint256 daysLeft;
        uint256 startDate;
        uint256 expiryDate;
        uint256 accruedPayout;
        AssetDepositType assetType;
        address asset;
    }

    #[derive(Debug, Serialize, Deserialize)]
    enum RiskType {
        Low,
        Medium,
        High
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Proposal {
        uint256 id;
        uint256 votesFor;
        uint256 votesAgainst;
        uint256 createdAt;
        uint256 deadline;
        uint256 timeleft;
        ProposalStaus status;
        bool executed;
        ProposalParams proposalParam;
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct ProposalParams {
        address user;
        RiskType riskType;
        uint256 coverId;
        string txHash;
        string description;
        uint256 poolId;
        uint256 claimAmount;
        AssetDepositType adt;
        address asset;
    }

    #[derive(Debug, Serialize, Deserialize)]
    interface IPool {
        function getUserGenericDeposit(uint256 _poolId, address _user, DepositType pdt) external view returns (GenericDepositDetails memory);
        function setUserDepositToZero(uint256 poolId, address user, DepositType pdt) external;
        function finalizeProposalClaim(uint256 _proposalId, address user) external;
    }

    interface IVault {
        function getUserVaultDeposit(uint256 vaultId, address user) external view returns (VaultDeposit memory);
        function setUserVaultDepositToZero(uint256 vaultId, address user) external;
    }

    interface IGov {
        function getProposalDetails(uint256 _proposalId) external returns (Proposal memory);
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static NONCE: RefCell<Option<u64>> = RefCell::new(None);
}

impl std::fmt::Debug for ERC20Token::balanceOfReturn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "balanceOfReturn({:?})", self)
    }
}

#[derive(CandidType, Deserialize, Default)]
struct State {
    owner: Option<Principal>,
    icp_contract_address: String,
    cover_contract: String,
    supported_networks: HashMap<Nat, Networks>, // chain id to the network RPC
}

#[init]
async fn init() {
    ic_cdk_timers::set_timer(Duration::from_secs(0), || {
        ic_cdk::spawn(async move {
            let signer = create_icp_signer().await;
            let address = signer.address();
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.icp_contract_address = address.to_string();
            });

            ic_cdk::println!("Initialising signer for address: {}", address);
        });
    });
}

fn next_id() -> u64 {
    thread_local! {
        static NEXT_ID: RefCell<u64> = RefCell::default();
    }
    NEXT_ID.with(|next_id| {
        let mut next_id = next_id.borrow_mut();
        let id = *next_id;
        *next_id = next_id.wrapping_add(1);
        id
    })
}

#[update(name = "getTotalTVL")]
async fn get_total_tvl() -> Result<Nat, String> {
    let networks = STATE.with(|state| {
        let state = state.borrow();
        state.supported_networks.clone()
    });

    let mut total_tvl: Nat = Nat::from(0u64);

    for (chain_id, network) in networks {
        match get_network_tvl(network.rpc_url.clone(), chain_id.clone()).await {
            Ok(network_balance) => total_tvl += network_balance,
            Err(e) => {
                ic_cdk::println!("Error getting TVL for chain {}: {}", chain_id, e);
            }
        }
    }

    Ok(total_tvl)
}

#[update(name = "getNetworkTVL")]
async fn get_network_tvl(new_network_rpc: String, chain_id: Nat) -> Result<Nat, String> {
    let (network, pool_contract_address) = STATE.with(|state| {
        let state = state.borrow();

        let network = match state.supported_networks.get(&chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {chain_id} not found"),
        };

        let pool_contract_address = match Address::from_str(&state.icp_contract_address) {
            Ok(address) => address,
            Err(_) => panic!("Error parsing pool contract address"),
        };

        (network.clone(), pool_contract_address)
    });

    let mut total_tvl: Nat = Nat::from(0u64);

    for asset in &network.supported_assets {
        let rpc_service = generate_rpc_service(new_network_rpc.clone());
        let config = IcpConfig::new(rpc_service);
        let provider = ProviderBuilder::new().on_icp(config);
        let balance = if asset == NULL_ADDRESS {
            let result = provider.get_balance(pool_contract_address).await;
            let balance = match result {
                Ok(value) => value,
                Err(e) => return Err(format!("Invalid balance type:{}", e)),
            };

            balance
        } else {
            let contract = ERC20Token::new(Address::from_str(asset).unwrap(), provider.clone());
            let result = contract.balanceOf(pool_contract_address).call().await;
            // let block = provider.get_block_number().await.unwrap();
            // println!("Current block number: {:?}", block);

            let balance = match result {
                Ok(value) => value.balance,
                Err(e) => return Err(format!("Invalid balance type:{}", e)),
            };

            balance
        };

        let balance_as_nat = Nat::from(balance.try_into().unwrap_or(u128::MAX));

        total_tvl += balance_as_nat;
    }

    Ok(total_tvl)
}

#[update(name = "Poolwithdraw")]
async fn pool_withdraw(
    pool_id: u64,
    user: String,
    pool_deposit_type: u8,
    chain_id: u64,
) -> Result<String, String> {
    let nat_chain_id = Nat::from(chain_id);

    let network = STATE.with(|state| {
        let state = state.borrow();

        let network = match state.supported_networks.get(&nat_chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {} not found", chain_id),
        };

        network.clone()
    });

    let pool_contract_address = match Address::from_str(&network.evm_pool_contract_address) {
        Ok(address) => address,
        Err(_) => panic!("Error parsing pool contract address"),
    };

    let user_address = match Address::from_str(&user) {
        Ok(address) => address,
        Err(e) => panic!("Error parsing user address {}", e),
    };

    let pdt = match pool_deposit_type {
        0 => DepositType::Normal,
        1 => DepositType::Vault,
        _ => return Err("Invalid deposit type".to_string()),
    };

    let signer = create_icp_signer().await;
    let address = signer.address();
    let wallet = EthereumWallet::from(signer);

    let rpc_service = generate_rpc_service(network.rpc_url.clone());
    let config = IcpConfig::new(rpc_service);
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_icp(config);
    let nonce = provider.get_transaction_count(address).await.unwrap_or(0);
    // let provider_chain_id = provider.get_chain_id().await.unwrap_or(chain_id);

    let pool_contract = InsurancePool::new(pool_contract_address, provider.clone());
    let result = pool_contract
        .getUserGenericDeposit(U256::from(pool_id), user_address, pdt.into())
        .call()
        .await;
    let (
        user,
        amount,
        pool_id,
        daily_payout,
        status,
        days_left,
        start_date,
        exp_date,
        accrued_payout,
        dt,
        adt,
        asset,
    ) = match result {
        Ok(value) => value._0,
        Err(e) => return Err(format!("Error fetching user deposit detials: {}", e)),
    };

    // let pool_call = IPool::getUserGenericDepositCall {
    //     _poolId: U256::from(pool_id),
    //     _user: user_address,
    //     pdt: pdt,
    // };

    // let call_data = pool_call.abi_encode();

    // let result =
    //     make_json_rpc_request(&pool_contract_address, call_data, network.rpc_url.clone()).await?;

    // let decoded_result = GenericDepositDetails::abi_decode(&result, false)
    //     .map_err(|e| format!("Failed to decode response: {}", e))?;
    // println!("Decoded Result: {:?}", decoded_result);

    let deposit_detail = GenericDepositDetail {
        lp: user,
        amount,
        pool_id,
        daily_payout,
        status,
        days_left,
        start_date,
        expiry_date: exp_date,
        accrued_payout,
        pdt: dt,
        adt,
        asset,
    };

    if pdt != DepositType::Normal {
        return Err("Must be pool withdrawal".to_string());
    }

    let gas_price = match provider.get_gas_price().await {
        Ok(price) => price,
        Err(_) => 10000000000,
    };

    match pool_contract
        .setUserDepositToZero(U256::from(pool_id), user_address, pdt.into())
        .nonce(nonce)
        .gas(200000)
        .gas_price(gas_price)
        .send()
        .await
    {
        Ok(_) => (),
        Err(e) => return Err(format!("Error setting user deposit to zero: {}", e)),
    }

    // let pool_set_call = IPool::setUserDepositToZeroCall {
    //     poolId: U256::from(pool_id),
    //     user: user_address,
    //     pdt: pdt,
    // };

    // let set_call_data = pool_set_call.abi_encode();

    // let _result = make_json_rpc_request(
    //     &pool_contract_address,
    //     set_call_data,
    //     network.rpc_url.clone(),
    // )
    // .await?;

    let signer = create_icp_signer().await;
    match deposit_detail.adt {
        0 => {
            let hash = match send_eth(
                signer,
                deposit_detail.amount,
                user_address,
                network.rpc_url,
                chain_id,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        1 => {
            let hash = match send_erc20_token(
                signer,
                deposit_detail.amount,
                user_address,
                network.rpc_url,
                chain_id,
                deposit_detail.asset,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        _ => {
            return Err(format!("Wrong Asset Deposit Type: {}", deposit_detail.adt));
        }
    }
}

#[update(name = "getNetworkDetails")]
async fn get_network_details(chain_id: u64) -> Result<NetworkDetails, String> {
    let nat_chain_id = Nat::from(chain_id);

    let network = STATE.with(|state| {
        let state = state.borrow();

        let network = match state.supported_networks.get(&nat_chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {} not found", chain_id),
        };

        network.clone()
    });

    let signer = create_icp_signer().await;
    let address = signer.address();
    let wallet = EthereumWallet::from(signer);

    let rpc_service = generate_rpc_service(network.rpc_url.clone());
    let config = IcpConfig::new(rpc_service);
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_icp(config);
    let nonce = provider.get_transaction_count(address).await.unwrap_or(0);
    let gas_price = provider.get_gas_price().await.unwrap_or(0);
    let gas = 1000;
    let provider_chain_id = provider.get_chain_id().await.unwrap_or(0);

    let netdet = NetworkDetails {
        chain_id: provider_chain_id,
        nonce,
        gas,
        gas_price,
    };

    Ok(netdet)
}

#[query(name = "getUserPoolDeposit")]
async fn get_user_pool_deposit(
    pool_id: u64,
    user: String,
    pool_deposit_type: u8,
    chain_id: u64,
) -> Result<UserDeposit, String> {
    let nat_chain_id = Nat::from(chain_id);

    let network = STATE.with(|state| {
        let state = state.borrow();

        let network = match state.supported_networks.get(&nat_chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {} not found", chain_id),
        };

        network.clone()
    });

    let pool_contract_address = match Address::from_str(&network.evm_pool_contract_address) {
        Ok(address) => address,
        Err(_) => panic!("Error parsing pool contract address"),
    };

    let user_address = match Address::from_str(&user) {
        Ok(address) => address,
        Err(e) => panic!("Error parsing user address {}", e),
    };

    let pdt = match pool_deposit_type {
        0 => DepositType::Normal,
        1 => DepositType::Vault,
        _ => return Err("Invalid deposit type".to_string()),
    };

    let pool_call = IPool::getUserGenericDepositCall {
        _poolId: U256::from(pool_id),
        _user: user_address,
        pdt: pdt,
    };

    let call_data = pool_call.abi_encode();

    let result =
        make_json_rpc_request(&pool_contract_address, call_data, network.rpc_url.clone()).await?;

    let decoded_result = GenericDepositDetails::abi_decode(&result, false)
        .map_err(|e| format!("Failed to decode response: {}", e))?;
    println!("Decoded Result: {:?}", decoded_result);

    let poolid_as_nat = Nat::from(decoded_result.poolId.try_into().unwrap_or(u128::MAX));

    let amount_as_nat = Nat::from(decoded_result.amount.try_into().unwrap_or(u128::MAX));

    let dailypayout_as_nat = Nat::from(decoded_result.dailyPayout.try_into().unwrap_or(u128::MAX));

    let days_left = Nat::from(decoded_result.daysLeft.try_into().unwrap_or(u128::MAX));

    let start_date = Nat::from(decoded_result.startDate.try_into().unwrap_or(u128::MAX));

    let expiry_date = Nat::from(decoded_result.expiryDate.try_into().unwrap_or(u128::MAX));

    let accrued_payout = Nat::from(decoded_result.daysLeft.try_into().unwrap_or(u128::MAX));

    let deposit_detail = UserDeposit {
        lp: Principal::from_str(&decoded_result.lp.to_string()).map_err(|e| e.to_string())?,
        amount: amount_as_nat,
        pool_id: poolid_as_nat,
        daily_payout: dailypayout_as_nat,
        days_left,
        start_date,
        expiry_date,
        accrued_payout,
        asset: decoded_result.asset.to_string(),
    };

    Ok(deposit_detail)
}

#[update(name = "vaultwithdraw")]
async fn vault_withdraw(
    vault_id: u64,
    user: String,
    pool_deposit_type: u8,
    chain_id: u64,
) -> Result<String, String> {
    let nat_chain_id = Nat::from(chain_id);

    let network = STATE.with(|state| {
        let state = state.borrow();

        let network = match state.supported_networks.get(&nat_chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {} not found", chain_id),
        };

        network.clone()
    });

    let vault_contract_address = match Address::from_str(&network.vault_contract) {
        Ok(address) => address,
        Err(_) => panic!("Error parsing vault contract address"),
    };

    let user_address = match Address::from_str(&user) {
        Ok(address) => address,
        Err(e) => panic!("Error parsing user address {}", e),
    };

    let pdt = match pool_deposit_type {
        0 => DepositType::Normal,
        1 => DepositType::Vault,
        _ => return Err("Invalid deposit type".to_string()),
    };

    let vault_call = IVault::getUserVaultDepositCall {
        vaultId: U256::from(vault_id),
        user: user_address,
    };

    let call_data = vault_call.abi_encode();

    let result =
        make_json_rpc_request(&vault_contract_address, call_data, network.rpc_url.clone()).await?;

    let decoded_result = VaultDeposit::abi_decode(&result, false)
        .map_err(|e| format!("Failed to decode response: {}", e))?;
    println!("Decoded Result: {:?}", decoded_result);

    if decoded_result.amount <= U256::from(0) {
        return Err(format!("No active deposit for user {}", decoded_result.lp));
    }

    let vault_deposit_detail = UserVaultDeposit {
        lp: decoded_result.lp,
        amount: decoded_result.amount,
        vault_id: decoded_result.vaultId,
        daily_payout: decoded_result.dailyPayout,
        status: match decoded_result.status {
            Status::Active => 0,
            Status::Due => 1,
            Status::Withdrawn => 2,
            _ => return Err("Invalid status value".to_string()),
        },
        days_left: decoded_result.daysLeft,
        start_date: decoded_result.startDate,
        expiry_date: decoded_result.expiryDate,
        accrued_payout: decoded_result.accruedPayout,
        adt: decoded_result.assetType.into(),
        asset: decoded_result.asset,
    };

    if pdt != DepositType::Vault {
        return Err("Must be vault withdrawal".to_string());
    }

    let vault_set_call = IVault::setUserVaultDepositToZeroCall {
        vaultId: U256::from(vault_id),
        user: user_address,
    };

    let set_call_data = vault_set_call.abi_encode();

    let _result = make_json_rpc_request(
        &vault_contract_address,
        set_call_data,
        network.rpc_url.clone(),
    )
    .await?;

    let signer = create_icp_signer().await;
    match vault_deposit_detail.adt {
        0 => {
            let hash = match send_eth(
                signer,
                vault_deposit_detail.amount,
                user_address,
                network.rpc_url,
                chain_id,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        1 => {
            let hash = match send_erc20_token(
                signer,
                vault_deposit_detail.amount,
                user_address,
                network.rpc_url,
                chain_id,
                vault_deposit_detail.asset,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        _ => {
            return Err(format!(
                "Wrong Asset Deposit Type: {}",
                vault_deposit_detail.adt
            ));
        }
    }
}

#[update(name = "claimProposalFunds")]
async fn claim_proposal_funds(
    proposal_id: u64,
    user: String,
    chain_id: u64,
) -> Result<String, String> {
    let nat_chain_id = Nat::from(chain_id);
    let network = STATE.with(|state| {
        let state = state.borrow();
        let network = match state.supported_networks.get(&nat_chain_id) {
            Some(network) => network,
            None => panic!("Network with chain_id {} not found", chain_id),
        };
        network.clone()
    });

    let gov_contract_address = match Address::from_str(&network.gov_address) {
        Ok(address) => address,
        Err(_) => panic!("Error parsing gov contract address"),
    };

    let pool_contract_address = match Address::from_str(&network.evm_pool_contract_address) {
        Ok(address) => address,
        Err(_) => panic!("Error parsing pool contract address"),
    };

    let user_address = match Address::from_str(&user) {
        Ok(address) => address,
        Err(e) => panic!("Error parsing user address {}", e),
    };

    let gov_call = IGov::getProposalDetailsCall {
        _proposalId: U256::from(proposal_id),
    };

    let call_data = gov_call.abi_encode();

    let result =
        make_json_rpc_request(&gov_contract_address, call_data, network.rpc_url.clone()).await?;

    let decoded_result = Proposal::abi_decode(&result, false)
        .map_err(|e| format!("Failed to decode response: {}", e))?;
    println!("Decoded Result: {:?}", decoded_result);

    if decoded_result.status != ProposalStaus::Approved {
        return Err(format!("Proposal is not approved"));
    }

    let pool_set_call = IPool::finalizeProposalClaimCall {
        _proposalId: U256::from(proposal_id),
        user: user_address,
    };

    let set_call_data = pool_set_call.abi_encode();

    let _result = make_json_rpc_request(
        &pool_contract_address,
        set_call_data,
        network.rpc_url.clone(),
    )
    .await?;

    let signer = create_icp_signer().await;
    match decoded_result.proposalParam.adt {
        AssetDepositType::Native => {
            let hash = match send_eth(
                signer,
                decoded_result.proposalParam.claimAmount,
                user_address,
                network.rpc_url,
                chain_id,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        AssetDepositType::ERC20 => {
            let hash = match send_erc20_token(
                signer,
                decoded_result.proposalParam.claimAmount,
                user_address,
                network.rpc_url,
                chain_id,
                decoded_result.proposalParam.asset,
            )
            .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    return Err(format!("Could not get transaction: {}", e));
                }
            };

            Ok(hash)
        }
        _ => {
            return Err(format!(
                "Wrong Asset Deposit Type: {:?}",
                decoded_result.proposalParam.adt
            ));
        }
    }
}

#[query(name = "getOwner")]
fn get_owner() -> Option<Principal> {
    STATE.with(|state| state.borrow().owner)
}

#[query(name = "getCanisterAddress")]
fn get_canister_address() -> Result<String, String> {
    let address = STATE.with(|state| state.borrow().icp_contract_address.clone());
    if address.is_empty() {
        return Err("Canister address not set".to_string());
    }

    Ok(address)
}

#[update(name = "setOwner")]
fn set_owner(new_owner: Principal) -> Result<(), String> {
    let caller = ic_cdk::caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.owner != Some(caller) {
            return Err("Only the current owner can set a new owner".to_string());
        }
        state.owner = Some(new_owner);
        Ok(())
    })
}

#[update(name = "addNewNetwork")]
fn add_new_network(
    new_network_rpc: String,
    chain_id: u64,
    network_name: String,
    assets: Vec<String>,
    pool_contract: String,
    gov_contract: String,
    cover_contract: String,
    vault_address: String,
) -> Result<(), String> {
    let nat_chain_id = Nat::from(chain_id);
    // let caller = ic_cdk::caller();
    let network = Networks {
        name: network_name.clone(),
        rpc_url: new_network_rpc.clone(),
        supported_assets: assets,
        cover_address: cover_contract,
        gov_address: gov_contract,
        vault_contract: vault_address,
        evm_pool_contract_address: pool_contract,
    };
    if new_network_rpc.trim().is_empty() || network_name.trim().is_empty() {
        return Err("RPC URL and network name cannot be empty".to_string());
    }
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        // if state.owner != Some(caller) {
        //     return Err("Only the current owner can add new network".to_string());
        // }
        if state.supported_networks.contains_key(&nat_chain_id) {
            return Err("Network with this chain_id already exists".to_string());
        }

        state.supported_networks.insert(nat_chain_id, network);
        Ok(())
    })
}

#[update(name = "updateNetwork")]
fn update_network(
    new_network_rpc: String,
    chain_id: u64,
    network_name: String,
    assets: Vec<String>,
    pool_contract: String,
    gov_contract: String,
    cover_contract: String,
    vault_address: String,
) -> Result<(), String> {
    let nat_chain_id = Nat::from(chain_id);

    if new_network_rpc.trim().is_empty() || network_name.trim().is_empty() {
        return Err("RPC URL and network name cannot be empty".to_string());
    }

    STATE.with(|state| {
        let mut state = state.borrow_mut();

        let network = state
            .supported_networks
            .get_mut(&nat_chain_id)
            .ok_or_else(|| format!("Network with chain ID {} does not exist", chain_id))?;

        network.name = network_name;
        network.rpc_url = new_network_rpc;
        network.supported_assets = assets;
        network.cover_address = cover_contract;
        network.gov_address = gov_contract;
        network.vault_contract = vault_address;
        network.evm_pool_contract_address = pool_contract;

        Ok(())
    })
}

#[query(name = "getNetworks")]
fn get_network(chain_id: u64) -> Result<Networks, String> {
    let nat_chain_id = Nat::from(chain_id);
    STATE.with(|state| {
        let state = state.borrow();
        state
            .supported_networks
            .get(&nat_chain_id)
            .cloned()
            .ok_or(format!("Network with chain_id {} not found", chain_id))
    })
}

#[update(name = "addNetworkAsset")]
fn add_network_asset(chain_id: u64, asset: String) -> Result<(), String> {
    let nat_chain_id = Nat::from(chain_id);
    let caller = ic_cdk::caller();
    if asset.trim().is_empty() {
        return Err("Asset cannot be empty".to_string());
    }

    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.owner != Some(caller) {
            return Err("Only the current owner can add new network".to_string());
        }
        let network = state
            .supported_networks
            .get_mut(&nat_chain_id)
            .ok_or(format!("Network with chain_id {} not found", chain_id))?;
        if !network.supported_assets.contains(&asset) {
            network.supported_assets.push(asset);
        }

        Ok(())
    })
}

async fn send_eth(
    signer: IcpSigner,
    amount: U256,
    to: Address,
    rpc_url: String,
    chain_id: u64,
) -> Result<String, String> {
    let address = signer.address();

    let wallet = EthereumWallet::from(signer);
    let rpc_service = generate_rpc_service(rpc_url);
    let config = IcpConfig::new(rpc_service);
    let provider = ProviderBuilder::new()
        .with_gas_estimation()
        .wallet(wallet)
        .on_icp(config);

    let maybe_nonce = NONCE.with_borrow(|maybe_nonce| maybe_nonce.map(|nonce| nonce + 1));

    let nonce = if let Some(nonce) = maybe_nonce {
        nonce
    } else {
        provider.get_transaction_count(address).await.unwrap_or(0)
    };

    let tx = TransactionRequest::default()
        .with_to(to)
        .with_value(amount)
        .with_nonce(nonce)
        .with_chain_id(chain_id);
    let transport_result = provider.send_transaction(tx.clone()).await;
    match transport_result {
        Ok(builder) => {
            let node_hash = *builder.tx_hash();
            let tx_response = provider.get_transaction_by_hash(node_hash).await.unwrap();

            match tx_response {
                Some(tx) => {
                    NONCE.with_borrow_mut(|nonce| {
                        *nonce = Some(tx.nonce);
                    });
                    Ok(format!("{:?}", tx.hash.to_string()))
                }
                None => Err("Could not get transaction.".to_string()),
            }
        }
        Err(e) => Err(format!("{:?}", e)),
    }
}

async fn send_erc20_token(
    signer: IcpSigner,
    amount: U256,
    to: Address,
    rpc_url: String,
    chain_id: u64,
    token_address: Address,
) -> Result<String, String> {
    let address = signer.address();

    let wallet = EthereumWallet::from(signer);
    let rpc_service = generate_rpc_service(rpc_url);
    let config = IcpConfig::new(rpc_service);
    let provider = ProviderBuilder::new()
        .with_gas_estimation()
        .wallet(wallet)
        .on_icp(config);

    let maybe_nonce = NONCE.with_borrow(|maybe_nonce| maybe_nonce.map(|nonce| nonce + 1));

    let nonce = if let Some(nonce) = maybe_nonce {
        nonce
    } else {
        provider.get_transaction_count(address).await.unwrap_or(0)
    };

    let contract = ERC20Token::new(token_address, provider.clone());

    match contract
        .transfer(to, amount)
        .nonce(nonce)
        .chain_id(chain_id)
        .from(address)
        .send()
        .await
    {
        Ok(builder) => {
            let node_hash = *builder.tx_hash();
            let tx_response = provider.get_transaction_by_hash(node_hash).await.unwrap();

            match tx_response {
                Some(tx) => {
                    NONCE.with_borrow_mut(|nonce| {
                        *nonce = Some(tx.nonce);
                    });
                    Ok(format!("{:?}", tx.hash.to_string()))
                }
                None => Err("Could not get transaction.".to_string()),
            }
        }
        Err(e) => Err(format!("{:?}", e)),
    }
}

async fn make_json_rpc_request(
    contract_address: &Address,
    call_data: Vec<u8>,
    rpc_url: String,
) -> Result<Vec<u8>, String> {
    let json_rpc_payload = serde_json::to_string(&JsonRpcRequest {
        id: next_id(),
        jsonrpc: "2.0".to_string(),
        method: "eth_call".to_string(),
        params: (
            EthCallParams {
                to: contract_address.to_string(),
                data: format!("0x{}", to_hex(&call_data)),
            },
            "latest".to_string(),
        ),
    })
    .map_err(|e| format!("Failed to serialize JSON-RPC request: {}", e))?;

    let request_headers = vec![HttpHeader {
        name: "Content-Type".to_string(),
        value: "application/json".to_string(),
    }];

    let request = CanisterHttpRequestArgument {
        url: rpc_url.clone(),
        max_response_bytes: Some(MAX_RESPONSE_BYTES),
        method: HttpMethod::POST,
        headers: request_headers,
        body: Some(json_rpc_payload.as_bytes().to_vec()),
        transform: Some(TransformContext::from_name("transform".to_string(), vec![])),
    };

    let (response,) = http_request(request, HTTP_CYCLES)
        .await
        .map_err(|(code, msg)| format!("HTTP request failed: {:?} {:?}", code, msg))?;

    let json: JsonRpcResult = serde_json::from_str(
        std::str::from_utf8(&response.body)
            .map_err(|e| format!("Failed to convert response to UTF-8: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse JSON response: {}", e))?;

    let _json_error = match json.error {
        Some(error) => return Err(format!("Json RPC error: {}", error.message)),
        None => (),
    };

    let raw_result = json
        .result
        .ok_or("No result in JSON-RPC response".to_string())?;

    let result = from_hex(&raw_result).map_err(|_| "Failed to decode response data".to_string())?;

    Ok(result)
}

#[update(name = "setPoolContractAddress")]
fn set_pool_contract(pool_contract: String) -> Result<(), String> {
    let caller = ic_cdk::caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.owner != Some(caller) {
            return Err("Only the current owner can set a new owner".to_string());
        }
        state.icp_contract_address = pool_contract;
        Ok(())
    })
}

#[query]
fn transform(args: TransformArgs) -> HttpResponse {
    transform_http_request(args)
}

ic_cdk::export_candid!();