//! Trasporto geo (port Fase 1 "coesistenza" da plenora-geo-tools-arrow).
//!
//! - [`protocol`]: framing v2 `PLNGEO2` checksummed per payload WKB;
//! - [`pair_protocol`]: framing `PLNPAIR1` per le coppie di indici dello
//!   spatial join;
//! - [`transport`]: trasporto Arrow v3 (envelope `PLNGEO3`, payload Arrow
//!   IPC, geometrie GeoArrow-WKB) con le operazioni batch del kernel;
//! - [`publish`]: verifica semantica del CRS e pubblicazione atomica
//!   dell'output (livello comandi del sorgente).

pub mod pair_protocol;
pub mod protocol;
pub mod publish;
pub mod transport;
