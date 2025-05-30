use ethers::prelude::Address;
use snowbridge_smoketest::{
	constants::*,
	contracts::{i_gateway_v1, i_gateway_v1::TokenTransferFeesChangedFilter},
	helper::*,
	parachains::{
		bridgehub,
		bridgehub::api::{
			ethereum_system::events::SetTokenTransferFees, runtime_types::primitive_types::U256,
		},
	},
};
use subxt::tx::Payload;

#[tokio::test]
async fn set_token_transfer_fees() {
	let test_clients = initial_clients().await.expect("initialize clients");

	let gateway_addr: Address = (*GATEWAY_PROXY_CONTRACT).into();
	let ethereum_client = *(test_clients.ethereum_client.clone());
	let gateway = i_gateway_v1::IGatewayV1::new(gateway_addr, ethereum_client.clone());
	let fees = gateway.quote_register_token_fee().await.expect("get fees");
	println!("register fees {:?}", fees);

	let ethereum_system_api = bridgehub::api::ethereum_system::calls::TransactionApi;

	let mut encoded = Vec::new();
	ethereum_system_api
		.set_token_transfer_fees(
			*CREATE_ASSET_FEE,
			*RESERVE_TRANSFER_FEE,
			U256([*REGISTER_TOKEN_FEE, 0, 0, 0]),
		)
		.encode_call_data_to(&test_clients.bridge_hub_client.metadata(), &mut encoded)
		.expect("encoded call");

	governance_bridgehub_call_from_relay_chain(encoded)
		.await
		.expect("set token fees");

	wait_for_bridgehub_event::<SetTokenTransferFees>(&test_clients.bridge_hub_client).await;

	wait_for_ethereum_event::<TokenTransferFeesChangedFilter>(&test_clients.ethereum_client).await;

	let fees = gateway.quote_register_token_fee().await.expect("get fees");
	println!("asset fees {:?}", fees);
}
