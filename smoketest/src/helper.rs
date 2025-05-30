use crate::{
	constants::*,
	contracts::i_gateway_v1 as i_gateway,
	parachains::{
		relaychain,
		relaychain::api::runtime_types::{
			pallet_xcm::pallet::Call as RelaychainPalletXcmCall,
			sp_weights::weight_v2::Weight as RelaychainWeight,
			staging_xcm::v3::multilocation::MultiLocation as RelaychainMultiLocation,
			westend_runtime::RuntimeCall as RelaychainRuntimeCall,
			xcm::{
				double_encoded::DoubleEncoded as RelaychainDoubleEncoded,
				v3::{
					junction::{
						Junction as RelaychainJunction,
						Junction::AccountId32 as RelaychainAccountId32,
						NetworkId as RelaychainNetworkId,
					},
					junctions::Junctions as RelaychainJunctions,
					multiasset::{
						AssetId as RelaychainAssetId, Fungibility as RelaychainFungibility,
						MultiAsset as RelaychainMultiAsset,
						MultiAssetFilter as RelaychainMultiAssetFilter,
						MultiAssets as RelaychainMultiAssets,
						WildMultiAsset as RelaychainWildMultiAsset,
					},
					Instruction as RelaychainInstruction, OriginKind as RelaychainOriginKind,
					WeightLimit as RelaychainWeightLimit, Xcm as RelaychainXcm,
				},
				VersionedLocation as RelaychainVersionedLocation,
				VersionedXcm as RelaychainVersionedXcm,
			},
		},
	},
};
use ethers::{
	prelude::{
		Address, EthEvent, LocalWallet, Middleware, Provider, Signer, SignerMiddleware,
		TransactionRequest, Ws, U256,
	},
	providers::Http,
	types::Log,
};
use futures::StreamExt;
use std::{ops::Deref, sync::Arc, time::Duration};
use subxt::{
	config::DefaultExtrinsicParams,
	events::StaticEvent,
	ext::sp_core::{sr25519::Pair, Pair as PairT},
	tx::PairSigner,
	utils::{AccountId32, MultiAddress, H256},
	Config, OnlineClient, PolkadotConfig,
};

/// Custom config that works with Statemint
pub enum AssetHubConfig {}

impl Config for AssetHubConfig {
	type Hash = <PolkadotConfig as Config>::Hash;
	type AccountId = <PolkadotConfig as Config>::AccountId;
	type Address = <PolkadotConfig as Config>::Address;
	type Signature = <PolkadotConfig as Config>::Signature;
	type Hasher = <PolkadotConfig as Config>::Hasher;
	type Header = <PolkadotConfig as Config>::Header;
	type ExtrinsicParams = DefaultExtrinsicParams<AssetHubConfig>;
	type AssetId = <PolkadotConfig as Config>::AssetId;
}

pub struct TestClients {
	pub asset_hub_client: Box<OnlineClient<AssetHubConfig>>,
	pub bridge_hub_client: Box<OnlineClient<PolkadotConfig>>,
	pub relaychain_client: Box<OnlineClient<PolkadotConfig>>,
	pub ethereum_client: Box<Arc<Provider<Ws>>>,
	pub ethereum_signed_client: Box<Arc<SignerMiddleware<Provider<Http>, LocalWallet>>>,
}

pub async fn initial_clients() -> Result<TestClients, Box<dyn std::error::Error>> {
	let bridge_hub_client: OnlineClient<PolkadotConfig> =
		OnlineClient::from_url((*BRIDGE_HUB_WS_URL).to_string())
			.await
			.expect("can not connect to bridgehub");

	let asset_hub_client: OnlineClient<AssetHubConfig> =
		OnlineClient::from_url((*ASSET_HUB_WS_URL).to_string())
			.await
			.expect("can not connect to assethub");

	let relaychain_client: OnlineClient<PolkadotConfig> =
		OnlineClient::from_url((*RELAY_CHAIN_WS_URL).to_string())
			.await
			.expect("can not connect to relaychain");

	let ethereum_provider = Provider::<Ws>::connect((*ETHEREUM_API).to_string())
		.await
		.unwrap()
		.interval(Duration::from_millis(10u64));

	let ethereum_client = Arc::new(ethereum_provider);

	let ethereum_signed_client = initialize_wallet().await.expect("initialize wallet");

	Ok(TestClients {
		asset_hub_client: Box::new(asset_hub_client),
		bridge_hub_client: Box::new(bridge_hub_client),
		relaychain_client: Box::new(relaychain_client),
		ethereum_client: Box::new(ethereum_client),
		ethereum_signed_client: Box::new(Arc::new(ethereum_signed_client)),
	})
}

pub async fn wait_for_bridgehub_event<Ev: StaticEvent>(
	bridge_hub_client: &Box<OnlineClient<PolkadotConfig>>,
) {
	let mut blocks = bridge_hub_client
		.blocks()
		.subscribe_finalized()
		.await
		.expect("block subscription")
		.take(500);

	let mut substrate_event_found = false;
	while let Some(Ok(block)) = blocks.next().await {
		println!("Polling bridgehub block {} for expected event.", block.number());
		let events = block.events().await.expect("read block events");
		for event in events.find::<Ev>() {
			let _ = event.expect("expect upgrade");
			println!("Event found at bridgehub block {}.", block.number());
			substrate_event_found = true;
			break
		}
		if substrate_event_found {
			break
		}
	}
	assert!(substrate_event_found);
}

pub async fn wait_for_assethub_event<Ev: StaticEvent>(
	asset_hub_client: &Box<OnlineClient<AssetHubConfig>>,
) {
	let mut blocks = asset_hub_client
		.blocks()
		.subscribe_finalized()
		.await
		.expect("block subscription")
		.take(5);

	let mut substrate_event_found = false;
	while let Some(Ok(block)) = blocks.next().await {
		println!("Polling assethub block {} for expected event.", block.number());
		let events = block.events().await.expect("read block events");
		for event in events.find::<Ev>() {
			let _ = event.expect("expect upgrade");
			println!(
				"Event found at assethub block {}: {}::{}",
				block.number(),
				<Ev as StaticEvent>::PALLET,
				<Ev as StaticEvent>::EVENT,
			);
			substrate_event_found = true;
			break
		}
		if substrate_event_found {
			break
		}
	}
	assert!(substrate_event_found);
}

pub async fn wait_for_ethereum_event<Ev: EthEvent>(ethereum_client: &Box<Arc<Provider<Ws>>>) {
	let gateway_addr: Address = (*GATEWAY_PROXY_CONTRACT).into();
	let gateway = i_gateway::IGatewayV1::new(gateway_addr, (*ethereum_client).deref().clone());

	let wait_for_blocks = 500;
	let mut stream = ethereum_client.subscribe_blocks().await.unwrap().take(wait_for_blocks);

	let mut ethereum_event_found = false;
	while let Some(block) = stream.next().await {
		println!("Polling ethereum block {:?} for expected event", block.number.unwrap());
		if let Ok(events) = gateway.event::<Ev>().at_block_hash(block.hash.unwrap()).query().await {
			for _ in events {
				println!("Event found at ethereum block {:?}", block.number.unwrap());
				ethereum_event_found = true;
				break
			}
		}
		if ethereum_event_found {
			break
		}
	}
	assert!(ethereum_event_found);
}

pub struct SudoResult {
	pub block_hash: H256,
	pub extrinsic_hash: H256,
}

pub async fn initialize_wallet(
) -> Result<SignerMiddleware<Provider<Http>, LocalWallet>, Box<dyn std::error::Error>> {
	let provider = Provider::<Http>::try_from((*ETHEREUM_HTTP_API).to_string())
		.unwrap()
		.interval(Duration::from_millis(10u64));

	let wallet: LocalWallet = (*ETHEREUM_KEY)
		.to_string()
		.parse::<LocalWallet>()
		.unwrap()
		.with_chain_id(ETHEREUM_CHAIN_ID);

	Ok(SignerMiddleware::new(provider.clone(), wallet.clone()))
}

pub async fn get_balance(
	client: &Box<Arc<SignerMiddleware<Provider<Http>, LocalWallet>>>,
	who: Address,
) -> Result<U256, Box<dyn std::error::Error>> {
	let balance = client.get_balance(who, None).await?;

	Ok(balance)
}

pub async fn fund_account(
	client: &Box<Arc<SignerMiddleware<Provider<Http>, LocalWallet>>>,
	address_to: Address,
	amount: u128,
) -> Result<(), Box<dyn std::error::Error>> {
	let tx = TransactionRequest::new()
		.to(address_to)
		.from(client.address())
		.value(U256::from(amount));
	let tx = client.send_transaction(tx, None).await?.await?;
	assert_eq!(tx.clone().unwrap().status.unwrap().as_u64(), 1u64);
	println!("receipt: {:#?}", hex::encode(tx.unwrap().transaction_hash));
	Ok(())
}

pub async fn governance_bridgehub_call_from_relay_chain(
	call: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");

	let sudo = Pair::from_string("//Alice", None).expect("cannot create sudo keypair");

	let signer: PairSigner<PolkadotConfig, _> = PairSigner::new(sudo);

	let weight = 180000000000;
	let proof_size = 900000;

	let dest = Box::new(RelaychainVersionedLocation::V3(RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(BRIDGE_HUB_PARA_ID)),
	}));
	let message = Box::new(RelaychainVersionedXcm::V3(RelaychainXcm(vec![
		RelaychainInstruction::UnpaidExecution {
			weight_limit: RelaychainWeightLimit::Unlimited,
			check_origin: None,
		},
		RelaychainInstruction::Transact {
			origin_kind: RelaychainOriginKind::Superuser,
			require_weight_at_most: RelaychainWeight { ref_time: weight, proof_size },
			call: RelaychainDoubleEncoded { encoded: call },
		},
	])));

	let sudo_api = relaychain::api::sudo::calls::TransactionApi;
	let sudo_call = sudo_api
		.sudo(RelaychainRuntimeCall::XcmPallet(RelaychainPalletXcmCall::send { dest, message }));

	let result = test_clients
		.relaychain_client
		.tx()
		.sign_and_submit_then_watch_default(&sudo_call, &signer)
		.await
		.expect("send through sudo call.")
		.wait_for_finalized_success()
		.await
		.expect("sudo call success");

	println!("Sudo call issued at relaychain block hash {:?}", result.extrinsic_hash());

	Ok(())
}

pub async fn snowbridge_assethub_call_from_relay_chain(
	call: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");

	let sudo = Pair::from_string("//Alice", None).expect("cannot create sudo keypair");

	let signer: PairSigner<PolkadotConfig, _> = PairSigner::new(sudo);

	let weight = 180000000000;
	let proof_size = 900000;

	let dest = Box::new(RelaychainVersionedLocation::V3(RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(ASSET_HUB_PARA_ID)),
	}));

	let message = Box::new(RelaychainVersionedXcm::V3(RelaychainXcm(vec![
		RelaychainInstruction::UnpaidExecution {
			weight_limit: RelaychainWeightLimit::Unlimited,
			check_origin: None,
		},
		RelaychainInstruction::DescendOrigin(RelaychainJunctions::X1(
			RelaychainJunction::Parachain(BRIDGE_HUB_PARA_ID),
		)),
		RelaychainInstruction::DescendOrigin(RelaychainJunctions::X1(
			RelaychainJunction::PalletInstance(INBOUND_QUEUE_PALLET_INDEX_V2),
		)),
		RelaychainInstruction::UniversalOrigin(RelaychainJunction::GlobalConsensus(
			RelaychainNetworkId::Ethereum { chain_id: ETHEREUM_CHAIN_ID },
		)),
		RelaychainInstruction::Transact {
			origin_kind: RelaychainOriginKind::SovereignAccount,
			require_weight_at_most: RelaychainWeight { ref_time: weight, proof_size },
			call: RelaychainDoubleEncoded { encoded: call },
		},
	])));

	let sudo_api = relaychain::api::sudo::calls::TransactionApi;
	let sudo_call = sudo_api
		.sudo(RelaychainRuntimeCall::XcmPallet(RelaychainPalletXcmCall::send { dest, message }));

	let result = test_clients
		.relaychain_client
		.tx()
		.sign_and_submit_then_watch_default(&sudo_call, &signer)
		.await
		.expect("send through sudo call.")
		.wait_for_finalized_success()
		.await
		.expect("sudo call success");

	println!("Sudo call issued at relaychain block hash {:?}", result.extrinsic_hash());

	Ok(())
}

pub async fn assethub_deposit_eth_on_penpal_call_from_relay_chain(
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");

	let sudo = Pair::from_string("//Alice", None).expect("cannot create sudo keypair");

	let signer: PairSigner<PolkadotConfig, _> = PairSigner::new(sudo);

	let weight = 180000000000;
	let proof_size = 900000;

	let dest = Box::new(RelaychainVersionedLocation::V3(RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(ASSET_HUB_PARA_ID)),
	}));

	let dot_location = RelaychainMultiLocation { parents: 1, interior: RelaychainJunctions::Here };
	let eth_location = RelaychainMultiLocation {
		parents: 2,
		interior: RelaychainJunctions::X1(RelaychainJunction::GlobalConsensus(
			RelaychainNetworkId::Ethereum { chain_id: ETHEREUM_CHAIN_ID },
		)),
	};

	let eth_asset: RelaychainMultiAsset = RelaychainMultiAsset {
		id: RelaychainAssetId::Concrete(eth_location),
		fun: RelaychainFungibility::Fungible(3_000_000_000_000u128),
	};
	let dot_asset: RelaychainMultiAsset = RelaychainMultiAsset {
		id: RelaychainAssetId::Concrete(dot_location),
		fun: RelaychainFungibility::Fungible(3_000_000_000_000u128),
	};

	let account_location: RelaychainMultiLocation = RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainAccountId32 {
			network: None,
			id: (*FERDIE_PUBLIC).into(),
		}),
	};

	let message = Box::new(RelaychainVersionedXcm::V3(RelaychainXcm(vec![
		RelaychainInstruction::UnpaidExecution {
			weight_limit: RelaychainWeightLimit::Unlimited,
			check_origin: None,
		},
		RelaychainInstruction::DescendOrigin(RelaychainJunctions::X1(
			RelaychainJunction::Parachain(BRIDGE_HUB_PARA_ID),
		)),
		RelaychainInstruction::DescendOrigin(RelaychainJunctions::X1(
			RelaychainJunction::PalletInstance(INBOUND_QUEUE_PALLET_INDEX_V2),
		)),
		RelaychainInstruction::UniversalOrigin(RelaychainJunction::GlobalConsensus(
			RelaychainNetworkId::Ethereum { chain_id: ETHEREUM_CHAIN_ID },
		)),
		RelaychainInstruction::WithdrawAsset(RelaychainMultiAssets(vec![RelaychainMultiAsset {
			id: RelaychainAssetId::Concrete(RelaychainMultiLocation {
				parents: 1,
				interior: RelaychainJunctions::Here,
			}),
			fun: RelaychainFungibility::Fungible(3_000_000_000_000_u128),
		}])),
		RelaychainInstruction::ReserveAssetDeposited(RelaychainMultiAssets(vec![
			RelaychainMultiAsset {
				id: RelaychainAssetId::Concrete(RelaychainMultiLocation {
					parents: 2,
					interior: RelaychainJunctions::X1(RelaychainJunction::GlobalConsensus(
						RelaychainNetworkId::Ethereum { chain_id: ETHEREUM_CHAIN_ID },
					)),
				}),
				fun: RelaychainFungibility::Fungible(3_000_000_000_000_u128),
			},
		])),
		RelaychainInstruction::DepositReserveAsset {
			// Send the token plus some eth for execution fees
			assets: RelaychainMultiAssetFilter::Definite(RelaychainMultiAssets(vec![
				dot_asset, eth_asset,
			])),
			// Penpal
			dest: RelaychainMultiLocation {
				parents: 1,
				interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(PENPAL_PARA_ID)),
			},
			xcm: RelaychainXcm(vec![
				// Pay fees on Penpal.
				RelaychainInstruction::BuyExecution {
					fees: RelaychainMultiAsset {
						id: RelaychainAssetId::Concrete(RelaychainMultiLocation {
							parents: 1,
							interior: RelaychainJunctions::Here,
						}),
						fun: RelaychainFungibility::Fungible(2_000_000_000_000_u128),
					},
					weight_limit: RelaychainWeightLimit::Limited(RelaychainWeight {
						ref_time: weight,
						proof_size,
					}),
				},
				// Deposit assets to beneficiary.
				RelaychainInstruction::DepositAsset {
					assets: RelaychainMultiAssetFilter::Wild(RelaychainWildMultiAsset::AllCounted(
						2,
					)),
					beneficiary: account_location,
				},
			]),
		},
	])));

	let sudo_api = relaychain::api::sudo::calls::TransactionApi;
	let sudo_call = sudo_api
		.sudo(RelaychainRuntimeCall::XcmPallet(RelaychainPalletXcmCall::send { dest, message }));

	let result = test_clients
		.relaychain_client
		.tx()
		.sign_and_submit_then_watch_default(&sudo_call, &signer)
		.await
		.expect("send through sudo call.")
		.wait_for_finalized_success()
		.await
		.expect("sudo call success");

	println!("Sudo call issued at relaychain block hash {:?}", result.extrinsic_hash());

	Ok(())
}

pub async fn governance_assethub_call_from_relay_chain_sudo_as(
	who: MultiAddress<AccountId32, ()>,
	call: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");

	let sudo = Pair::from_string("//Alice", None).expect("cannot create sudo keypair");

	let signer: PairSigner<PolkadotConfig, _> = PairSigner::new(sudo);

	let weight = 180000000000;
	let proof_size = 900000;

	let dest = Box::new(RelaychainVersionedLocation::V3(RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(ASSET_HUB_PARA_ID)),
	}));

	let message = Box::new(RelaychainVersionedXcm::V3(RelaychainXcm(vec![
		RelaychainInstruction::BuyExecution {
			fees: RelaychainMultiAsset {
				id: RelaychainAssetId::Concrete(RelaychainMultiLocation {
					parents: 0,
					interior: RelaychainJunctions::Here,
				}),
				fun: RelaychainFungibility::Fungible(7_000_000_000_000_u128),
			},
			weight_limit: RelaychainWeightLimit::Limited(RelaychainWeight {
				ref_time: weight,
				proof_size,
			}),
		},
		RelaychainInstruction::Transact {
			origin_kind: RelaychainOriginKind::Superuser,
			require_weight_at_most: RelaychainWeight { ref_time: weight, proof_size },
			call: RelaychainDoubleEncoded { encoded: call },
		},
	])));

	let sudo_api = relaychain::api::sudo::calls::TransactionApi;
	let sudo_call = sudo_api.sudo_as(
		who,
		RelaychainRuntimeCall::XcmPallet(RelaychainPalletXcmCall::send { dest, message }),
	);

	let result = test_clients
		.relaychain_client
		.tx()
		.sign_and_submit_then_watch_default(&sudo_call, &signer)
		.await
		.expect("send through sudo call.")
		.wait_for_finalized_success()
		.await
		.expect("sudo call success");

	println!("Sudo call issued at relaychain block hash {:?}", result.extrinsic_hash());

	Ok(())
}

pub async fn fund_agent(
	agent_id: [u8; 32],
	amount: u128,
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");
	let gateway_addr: Address = (*GATEWAY_PROXY_CONTRACT).into();
	let ethereum_client = *(test_clients.ethereum_client.clone());
	let gateway = i_gateway::IGatewayV1::new(gateway_addr, ethereum_client.clone());
	let agent_address = gateway.agent_of(agent_id).await.expect("find agent");

	println!("agent address {}", hex::encode(agent_address));

	fund_account(&test_clients.ethereum_signed_client, agent_address, amount)
		.await
		.expect("fund account");
	Ok(())
}

pub fn print_event_log_for_unit_tests(log: &Log) {
	let topics: Vec<String> = log.topics.iter().map(|t| hex::encode(t.as_ref())).collect();
	println!("Log {{");
	println!("	address: hex!(\"{}\").into(),", hex::encode(log.address.as_ref()));
	println!("	topics: vec![");
	for topic in topics.iter() {
		println!("		hex!(\"{}\").into(),", topic);
	}
	println!("	],");
	println!("	data: hex!(\"{}\").into(),", hex::encode(&log.data));

	println!("}}")
}

pub async fn governance_assethub_call_from_relay_chain(
	call: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
	let test_clients = initial_clients().await.expect("initialize clients");

	let sudo = Pair::from_string("//Alice", None).expect("cannot create sudo keypair");

	let signer: PairSigner<PolkadotConfig, _> = PairSigner::new(sudo);

	let weight = 180000000000;
	let proof_size = 900000;

	let dest = Box::new(RelaychainVersionedLocation::V3(RelaychainMultiLocation {
		parents: 0,
		interior: RelaychainJunctions::X1(RelaychainJunction::Parachain(ASSET_HUB_PARA_ID)),
	}));
	let message = Box::new(RelaychainVersionedXcm::V3(RelaychainXcm(vec![
		RelaychainInstruction::UnpaidExecution {
			weight_limit: RelaychainWeightLimit::Unlimited,
			check_origin: None,
		},
		RelaychainInstruction::Transact {
			origin_kind: RelaychainOriginKind::Superuser,
			require_weight_at_most: RelaychainWeight { ref_time: weight, proof_size },
			call: RelaychainDoubleEncoded { encoded: call },
		},
	])));

	let sudo_api = relaychain::api::sudo::calls::TransactionApi;
	let sudo_call = sudo_api
		.sudo(RelaychainRuntimeCall::XcmPallet(RelaychainPalletXcmCall::send { dest, message }));

	let result = test_clients
		.relaychain_client
		.tx()
		.sign_and_submit_then_watch_default(&sudo_call, &signer)
		.await
		.expect("send through sudo call.")
		.wait_for_finalized_success()
		.await
		.expect("sudo call success");

	println!("Sudo call issued at relaychain block hash {:?}", result.extrinsic_hash());

	Ok(())
}
