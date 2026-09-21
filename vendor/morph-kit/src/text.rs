//! Petits utilitaires de texte communs aux outils.

use serde_json::Value;

/// Un paramètre texte obligatoire et non vide.
pub fn text_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Paramètre `{key}` requis."))
}

/// Les `n` premiers caractères.
pub fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_string)
        .collect()
}

/// Nombre de mots de `goal` retrouvés dans `texte`.
pub fn lexical_score(goal: &str, texte: &str) -> usize {
    let bas = texte.to_lowercase();
    tokens(goal)
        .iter()
        .filter(|w| bas.contains(w.as_str()))
        .count()
}

/// Les `n` éléments les plus proches de l'objectif par les mots ; `bonus`
/// départage à égalité (par exemple ce qui est à l'écran).
pub fn shortlist<T: Clone>(
    goal: &str,
    items: &[T],
    n: usize,
    text: impl Fn(&T) -> String,
    bonus: impl Fn(&T) -> usize,
) -> Vec<T> {
    let mut scored: Vec<(usize, &T)> = items
        .iter()
        .map(|it| (lexical_score(goal, &text(it)) * 2 + bonus(it), it))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(n)
        .map(|(_, it)| it.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn un_parametre_vide_est_refuse() {
        assert!(text_arg(&json!({"goal":"  "}), "goal").is_err());
        assert_eq!(text_arg(&json!({"goal":" x "}), "goal").unwrap(), "x");
    }

    #[test]
    fn les_mots_communs_se_comptent() {
        assert_eq!(
            lexical_score("ajouter panier", "Bouton Ajouter au panier"),
            2
        );
        assert_eq!(lexical_score("de la", "Accueil"), 0);
    }

    #[test]
    fn la_preselection_garde_les_plus_proches() {
        let items = vec!["Accueil", "Ajouter au panier"];
        let r = shortlist("ajouter panier", &items, 1, |s| s.to_string(), |_| 0);
        assert_eq!(r, vec!["Ajouter au panier"]);
    }
}
