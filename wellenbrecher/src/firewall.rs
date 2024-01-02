use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};

use nftables::batch::Batch;
use nftables::expr::{BinaryOperation, Expression, NamedExpression, Payload, CT};
use nftables::helper::apply_ruleset;
use nftables::schema::{Chain, FlushObject, NfCmd, NfListObject, Rule, Table};
use nftables::stmt::{CTCount, Match, Meter, Operator, Statement};
use nftables::types::{NfChainPolicy, NfChainType, NfFamily, NfHook};
use tracing::{debug, enabled, error, trace, Level};

const NFT_TABLE_NAME: &str = "WELLENBRECHER_FILTER";
const NFT_IN_CHAIN_NAME: &str = "WELLENBRECHER_IN_CHAIN";

pub struct ConnectionLimit {
    applied_once: AtomicBool,
    port: u16,
    connections_per_ip: u32,
    table_ipv4: Table,
    table_ipv6: Table,
    in_chain_ipv4: Chain,
    in_chain_ipv6: Chain,
    ipv4_mask: Ipv4Addr,
    ipv6_mask: Ipv6Addr,
}

impl ConnectionLimit {
    pub fn new(
        port: u16,
        connections_per_ip: u32,
        ipv4_mask: Ipv4Addr,
        ipv6_mask: Ipv6Addr,
    ) -> Self {
        let table_ipv4 = Table::new(NfFamily::IP, String::from(NFT_TABLE_NAME));
        let table_ipv6 = Table::new(NfFamily::IP6, String::from(NFT_TABLE_NAME));

        let in_chain_ipv4 = Chain::new(
            table_ipv4.family.clone(),
            table_ipv4.name.clone(),
            String::from(NFT_IN_CHAIN_NAME),
            NfChainType::Filter.into(),
            NfHook::Input.into(),
            Some(100),
            None,
            NfChainPolicy::Accept.into(),
        );
        let in_chain_ipv6 = Chain {
            family: table_ipv6.family.clone(),
            ..in_chain_ipv4.clone()
        };

        Self {
            applied_once: AtomicBool::new(false),
            port,
            connections_per_ip,
            ipv4_mask,
            ipv6_mask,
            table_ipv4,
            table_ipv6,
            in_chain_ipv4,
            in_chain_ipv6,
        }
    }

    pub fn apply(&self) -> Result<(), nftables::helper::NftablesError> {
        let mut batch = Batch::new();
        batch.add_cmd(NfCmd::Add(NfListObject::Table(self.table_ipv4.clone())));
        batch.add_cmd(NfCmd::Add(NfListObject::Table(self.table_ipv6.clone())));
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(self.table_ipv4.clone())));
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(self.table_ipv6.clone())));

        batch.add_cmd(NfCmd::Add(NfListObject::Chain(self.in_chain_ipv4.clone())));
        batch.add_cmd(NfCmd::Add(NfListObject::Chain(self.in_chain_ipv6.clone())));

        let add_limit_rule =
            |batch: &mut Batch, table: &Table, chain: &Chain, proto: &str, mask: String| {
                batch.add_cmd(NfCmd::Add(NfListObject::Rule(Rule {
                    family: table.family.clone(),
                    table: table.name.clone(),
                    chain: chain.name.clone(),
                    expr: vec![
                        Statement::Match(Match {
                            op: Operator::EQ,
                            left: Expression::Named(NamedExpression::Payload(Payload {
                                protocol: "tcp".to_string(),
                                field: "dport".to_string(),
                            })),
                            right: Expression::Number(self.port as u32),
                        }),
                        Statement::Match(Match {
                            op: Operator::EQ,
                            left: Expression::Named(NamedExpression::CT(CT {
                                family: None,
                                dir: None,
                                key: "state".to_string(),
                            })),
                            right: Expression::String("new".to_string()),
                        }),
                        Statement::Meter(Meter {
                            name: "CONNECTION_METER".to_string(),
                            key: Expression::BinaryOperation(BinaryOperation::AND(
                                Box::new(Expression::Named(NamedExpression::Payload(Payload {
                                    protocol: proto.to_string(),
                                    field: "saddr".to_string(),
                                }))),
                                Box::new(Expression::String(mask.to_string())),
                            )),
                            stmt: Box::new(Statement::CTCount(CTCount {
                                val: Expression::Number(self.connections_per_ip),
                                inv: Some(true),
                            })),
                        }),
                        Statement::Reject(None),
                    ],
                    handle: None,
                    index: None,
                    comment: None,
                })));
            };

        add_limit_rule(
            &mut batch,
            &self.table_ipv4,
            &self.in_chain_ipv4,
            "ip",
            self.ipv4_mask.to_string(),
        );
        add_limit_rule(
            &mut batch,
            &self.table_ipv6,
            &self.in_chain_ipv6,
            "ip6",
            self.ipv6_mask.to_string(),
        );

        let args = if enabled!(Level::TRACE) {
            Some(vec!["-d", "all"])
        } else {
            None
        };

        let result = apply_ruleset(&batch.to_nftables(), None, args);
        _ = self
            .applied_once
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |applied_once| {
                Some(applied_once | result.is_ok())
            })
            .unwrap();
        result
    }
}

impl Drop for ConnectionLimit {
    fn drop(&mut self) {
        if !*self.applied_once.get_mut() {
            return;
        }

        trace!("removing nftables rules…");

        let mut batch = Batch::new();
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(self.table_ipv4.clone())));
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(self.table_ipv6.clone())));
        if let Err(e) = apply_ruleset(&batch.to_nftables(), None, None) {
            error!("unable to remove nftables rules: {e}");
        } else {
            debug!("nftables rules removed");
        }
    }
}
