use super::*;
use crate::common::{committee, qc};
use crypto::generate_keypair;
use rand::rngs::StdRng;
use rand::SeedableRng as _;

#[test]
fn verify_valid_qc() {
    let mut committees = Committees::new();
    committees.add_committe_for_epoch(committee(), 1);

    assert!(qc().verify(&committees).is_ok());
}

#[test]
fn verify_qc_authority_reuse() {
    let mut committees = Committees::new();
    committees.add_committe_for_epoch(committee(), 1);

    // Modify QC to reuse one authority.
    let mut qc = qc();
    let _ = qc.votes.pop();
    let vote = qc.votes[0].clone();
    qc.votes.push(vote.clone());

    // Verify the QC.
    match qc.verify(&committees) {
        Err(ConsensusError::AuthorityReuse(name)) => assert_eq!(name, vote.0),
        _ => assert!(false),
    }
}

#[test]
fn verify_qc_unknown_authority() {
    let mut committees = Committees::new();
    committees.add_committe_for_epoch(committee(), 1);

    let mut qc = qc();

    // Modify QC to add one unknown authority.
    let mut rng = StdRng::from_seed([1; 32]);
    let (unknown, _) = generate_keypair(&mut rng);
    let (_, sig) = qc.votes.pop().unwrap();
    qc.votes.push((unknown, sig));

    // Verify the QC.
    match qc.verify(&committees) {
        Err(ConsensusError::UnknownAuthority(name)) => assert_eq!(name, unknown),
        _ => assert!(false),
    }
}

#[test]
fn verify_qc_insufficient_stake() {
    let mut committees = Committees::new();
    committees.add_committe_for_epoch(committee(), 1);

    // Modify QC to remove one authority.
    let mut qc = qc();
    let _ = qc.votes.pop();

    // Verify the QC.
    match qc.verify(&committees) {
        Err(ConsensusError::QCRequiresQuorum) => assert!(true),
        _ => assert!(false),
    }
}
