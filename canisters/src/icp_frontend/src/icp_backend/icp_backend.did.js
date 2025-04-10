export const idlFactory = ({ IDL }) => {
  const Result = IDL.Variant({ 'Ok' : IDL.Text, 'Err' : IDL.Text });
  const Result_1 = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text });
  const NetworkDetails = IDL.Record({
    'gas' : IDL.Nat,
    'chain_id' : IDL.Nat64,
    'nonce' : IDL.Nat64,
    'gas_price' : IDL.Nat,
  });
  const Result_2 = IDL.Variant({ 'Ok' : NetworkDetails, 'Err' : IDL.Text });
  const Result_3 = IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : IDL.Text });
  const Networks = IDL.Record({
    'vault_contract' : IDL.Text,
    'name' : IDL.Text,
    'gov_address' : IDL.Text,
    'evm_pool_contract_address' : IDL.Text,
    'cover_address' : IDL.Text,
    'rpc_url' : IDL.Text,
    'supported_assets' : IDL.Vec(IDL.Text),
  });
  const Result_4 = IDL.Variant({ 'Ok' : Networks, 'Err' : IDL.Text });
  const UserDeposit = IDL.Record({
    'lp' : IDL.Principal,
    'asset' : IDL.Text,
    'accrued_payout' : IDL.Nat,
    'start_date' : IDL.Nat,
    'expiry_date' : IDL.Nat,
    'days_left' : IDL.Nat,
    'pool_id' : IDL.Nat,
    'daily_payout' : IDL.Nat,
    'amount' : IDL.Nat,
  });
  const Result_5 = IDL.Variant({ 'Ok' : UserDeposit, 'Err' : IDL.Text });
  const HttpHeader = IDL.Record({ 'value' : IDL.Text, 'name' : IDL.Text });
  const HttpResponse = IDL.Record({
    'status' : IDL.Nat,
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(HttpHeader),
  });
  const TransformArgs = IDL.Record({
    'context' : IDL.Vec(IDL.Nat8),
    'response' : HttpResponse,
  });
  return IDL.Service({
    'Poolwithdraw' : IDL.Func(
        [IDL.Nat64, IDL.Text, IDL.Nat8, IDL.Nat64],
        [Result],
        [],
      ),
    'addNetworkAsset' : IDL.Func([IDL.Nat64, IDL.Text], [Result_1], []),
    'addNewNetwork' : IDL.Func(
        [
          IDL.Text,
          IDL.Nat64,
          IDL.Text,
          IDL.Vec(IDL.Text),
          IDL.Text,
          IDL.Text,
          IDL.Text,
          IDL.Text,
        ],
        [Result_1],
        [],
      ),
    'claimProposalFunds' : IDL.Func(
        [IDL.Nat64, IDL.Text, IDL.Nat64],
        [Result],
        [],
      ),
    'getCanisterAddress' : IDL.Func([], [Result], ['query']),
    'getNetworkDetails' : IDL.Func([IDL.Nat64], [Result_2], []),
    'getNetworkTVL' : IDL.Func([IDL.Text, IDL.Nat], [Result_3], []),
    'getNetworks' : IDL.Func([IDL.Nat64], [Result_4], ['query']),
    'getOwner' : IDL.Func([], [IDL.Opt(IDL.Principal)], ['query']),
    'getTotalTVL' : IDL.Func([], [Result_3], []),
    'getUserPoolDeposit' : IDL.Func(
        [IDL.Nat64, IDL.Text, IDL.Nat8, IDL.Nat64],
        [Result_5],
        ['query'],
      ),
    'setOwner' : IDL.Func([IDL.Principal], [Result_1], []),
    'setPoolContractAddress' : IDL.Func([IDL.Text], [Result_1], []),
    'transform' : IDL.Func([TransformArgs], [HttpResponse], ['query']),
    'updateNetwork' : IDL.Func(
        [
          IDL.Text,
          IDL.Nat64,
          IDL.Text,
          IDL.Vec(IDL.Text),
          IDL.Text,
          IDL.Text,
          IDL.Text,
          IDL.Text,
        ],
        [Result_1],
        [],
      ),
    'vaultwithdraw' : IDL.Func(
        [IDL.Nat64, IDL.Text, IDL.Nat8, IDL.Nat64],
        [Result],
        [],
      ),
  });
};
export const init = ({ IDL }) => { return []; };
