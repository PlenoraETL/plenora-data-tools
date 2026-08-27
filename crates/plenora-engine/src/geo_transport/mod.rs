//! Trasporto geo (port Fase 1 "coesistenza" da plenora-geo-tools-arrow).
//!
//! - [`framing`]: header e chiusura, le sole parti che i tre stream
//!   checksummati hanno davvero in comune;
//! - [`protocol`]: framing v2 `PLNGEO2` checksummed per payload WKB;
//! - [`pair_protocol`]: framing `PLNPAIR1` per le coppie di indici dello
//!   spatial join;
//! - [`transport`]: trasporto Arrow v3 (envelope `PLNGEO3`, payload Arrow
//!   IPC, geometrie GeoArrow-WKB) con le operazioni batch del kernel;
//! - [`publish`]: verifica semantica del CRS e pubblicazione atomica
//!   dell'output (livello comandi del sorgente).

pub(crate) mod envelope;
pub(crate) mod error;
pub(crate) mod framing;
pub(crate) mod ipc;
pub(crate) mod pair;
pub mod pair_protocol;
pub mod protocol;
pub mod publish;
pub(crate) mod schema;
pub mod transport;
pub(crate) mod unary;
