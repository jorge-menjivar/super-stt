use super::*;
use std::collections::{HashMap, HashSet};

fn loader_for(language: &str) -> FluentLanguageLoader {
    let loader = fluent_language_loader!();
    i18n_embed::select(&loader, &Localizations, &[language.parse().unwrap()]).unwrap();
    loader
}

#[test]
fn french_regional_locale_and_english_fallback() {
    let french = loader_for("fr-FR");
    assert_eq!(french.get("models"), "Modèles");
    assert_eq!(french.get("recording"), "Enregistrement");
    assert_eq!(french.get("stop-manual"), "Arrêt manuel uniquement");
    assert_eq!(loader_for("en").get("models"), "Models");
    assert_eq!(loader_for("zz").get("models"), "Models");
}

#[test]
fn french_dynamic_message_preserves_model_name() {
    let loader = loader_for("fr");
    let args = HashMap::from([("target_model", "whisper-small")]);
    assert_eq!(
        loader.get_args("loading-model", args),
        "Chargement de \u{2068}whisper-small\u{2069}"
    );
}

#[test]
fn french_catalog_covers_english_messages_and_variables() {
    fn messages(path: &str) -> HashMap<String, HashSet<String>> {
        let resource = Localizations::get(path).unwrap();
        let text = std::str::from_utf8(&resource.data).unwrap();
        let mut messages = HashMap::new();
        let mut current = None;
        for line in text.lines() {
            if !line.starts_with(char::is_whitespace)
                && let Some((id, _)) = line.split_once(" = ")
            {
                assert!(
                    messages.insert(id.to_string(), HashSet::new()).is_none(),
                    "duplicate {id}"
                );
                current = Some(id.to_string());
            }
            if let Some(id) = &current {
                for part in line.split('$').skip(1) {
                    let name: String = part
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    messages.get_mut(id).unwrap().insert(name);
                }
            }
        }
        messages
    }
    assert_eq!(
        messages("en/super_stt_app.ftl"),
        messages("fr/super_stt_app.ftl")
    );
}
