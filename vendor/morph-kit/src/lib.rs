//! Briques partagées par `morph-browser` et `morph-desktop`.
//!
//! Chaque morph vit dans son propre dépôt et ne partage rien avec les autres
//! par le socle ; cette caisse évite de recopier deux fois la boucle MCP et le
//! pont vers Laya.

pub mod laya;
pub mod mcp;
pub mod risk;
pub mod text;
