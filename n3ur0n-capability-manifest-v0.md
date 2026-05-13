# N3UR0N — Capability Manifest v0.1 (brouillon)

**Date** : 2026-05-12
**Statut** : brouillon de spec, non implémenté. À discuter avant gel.
**Objet** : définir le format `cap.toml` qui déclare une capacité exposée par une instance n3uron.
**Périmètre** : v0.1 — couvre les bindings MCP, prompted-LLM, HTTP-forward. WASM, subprocess générique et binding composé sont hors-périmètre.
**Lecteur** : implémenteur de la v0.2, publisher écrivant sa première cap, reviewer du format.

---

## 0. Principe directeur

Une capacité est *décrite chez n3uron, exécutée ailleurs*. Le fichier `cap.toml` est la **source unique de vérité** pour ce que N3UR0N expose au réseau : nom, description, schémas, exemples, mode d'accès, lobes. Aucune de ces informations n'est dérivée d'une source externe — n3uron ne va pas lire le `description` d'un tool MCP pour fabriquer un `describe_self`. Le publisher est responsable de la métadonnée éditoriale ; le binding ne fait que définir *où* l'invocation est envoyée.

Cette discipline a un coût (un publisher qui wrappe un MCP server tiers doit retaper la description en français N3UR0N) et un bénéfice (le publisher reste maître de comment sa cap se présente, et la métadonnée ne dérive jamais silencieusement quand le backend change).

Convention de localisation : un manifeste par fichier, un fichier par cap. Watcher sur `~/.n3ur0n/caps/*.toml` (configurable). Pas de manifestes multi-caps : la lisibilité prime sur la concision.

---

## 1. Structure générale

```toml
# Métadonnée du fichier lui-même (pas de la cap)
[manifest]
version = "0.1"        # version du format cap.toml

# Métadonnée publique de la capacité (devient CapabilityDecl)
[descriptor]
name = "translator-fr-en"
description = "Traduit du texte court du français vers l'anglais avec un style neutre."
mode = "free"
tags = ["translation", "language", "fr", "en"]
lobe_ids = ["lobe.community.translators.v1"]
pricing = "free"        # chaîne libre en v0.1

# Schémas — déclarés ici, validés contre le binding au démarrage
[descriptor.schema_in]
type = "object"
required = ["text"]
properties = { text = { type = "string", maxLength = 4000 } }

[descriptor.schema_out]
type = "object"
required = ["translation"]
properties = { translation = { type = "string" } }

# Exemples canoniques (forte recommandation, non bloquant en v0.1)
[[descriptor.examples]]
intent = "traduire 'bonjour' en anglais"
args = { text = "bonjour" }
expected_output = { translation = "hello" }

[[descriptor.examples]]
intent = "traduire une phrase technique courte"
args = { text = "Le serveur est en panne." }
expected_output = { translation = "The server is down." }

# Disambiguation (recommandé dès qu'il existe des caps voisines)
disambiguation = """
Préférer cette cap à `chat` pour une simple traduction littérale fr→en.
Ne pas utiliser pour : traduction vers d'autres langues, traduction de
documents longs (>4000 caractères), réécriture stylistique.
"""

# Contre-exemples (optionnels mais utiles pour le planner)
[[descriptor.negative_examples]]
intent = "résume ce texte en anglais"
why_not = "Cette cap traduit littéralement, ne résume pas. Utiliser `summarizer-en` puis cette cap si besoin."

# Binding : comment invoquer effectivement
[binding]
type = "mcp"            # mcp | prompted_llm | http

[binding.mcp]
server = "command:translator-mcp --port 5005"
tool_name = "translate_fr_en"
# arg_mapping facultatif si les noms diffèrent
# Par défaut, args N3UR0N transmis tels quels au tool MCP.

# Cycle de vie (optionnel ; defaults raisonnables si absent)
[lifecycle]
warmup = false          # connecter le binding au boot vs à la première invocation
timeout_ms = 30000
```

Tous les champs hors `manifest.version`, `descriptor.name`, `descriptor.description`, `descriptor.mode`, `descriptor.schema_in`, `descriptor.schema_out`, `binding.type` et le sous-bloc `[binding.<type>]` sont *optionnels*. Les champs optionnels marqués "fortement recommandés" déclenchent un warning au load mais n'empêchent pas l'activation.

---

## 2. Référence par section

### 2.1 `[manifest]`

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `version` | string | oui | Version du format cap.toml. v0.1 attend `"0.1"`. Permet à n3uron de refuser un manifeste de format futur. |

### 2.2 `[descriptor]`

Tous ces champs deviennent les champs publics de `CapabilityDecl` exposés via `describe_self`.

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `name` | string | oui | Nom unique au sein de l'instance. ASCII, `[a-z0-9_.-]+`, ≤ 64 caractères. |
| `description` | string | oui | Description courte (≤ 280 caractères recommandé). Première information vue par les planners distants. |
| `mode` | `"free"` ou `"restricted"` | oui | Mode d'accès. `restricted` exige whitelist ou subscription_token (logique côté n3uron). |
| `tags` | array<string> | non | Tags libres pour discovery. |
| `lobe_ids` | array<string> | non | Lobes auxquels la cap est attachée. |
| `pricing` | string | non | Chaîne libre. Convention v0.1 : `"free"`, `"per-invocation:0.001USD"`, etc. Pas de parsing en v0.1. |
| `disambiguation` | string (multiline) | recommandé | Texte expliquant quand préférer / éviter cette cap. Lu par le planner. |
| `schema_in` | objet JSON Schema | oui | Schéma d'entrée. Validé contre le binding au boot. |
| `schema_out` | objet JSON Schema | oui | Schéma de sortie. Validé contre le binding au boot. Toute sortie qui ne valide pas est rejetée à l'exécution. |

### 2.3 `[[descriptor.examples]]` (tableau de tables)

Recommandé fortement. Chaque exemple a :

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `intent` | string | oui | Intention utilisateur en langage naturel. Sert au planner pour matcher. |
| `args` | objet | oui | Args concrets correspondant à l'intention. Validé contre `schema_in`. |
| `expected_output` | objet | recommandé | Sortie attendue, à titre indicatif. Sert aussi pour `n3ur0n cap test`. |

### 2.4 `[[descriptor.negative_examples]]`

Optionnel mais utile pour disambiguer face à des caps voisines.

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `intent` | string | oui | Intention utilisateur qui *pourrait* sembler matcher. |
| `why_not` | string | oui | Pourquoi cette cap n'est pas la bonne, et laquelle préférer si connue. |

### 2.5 `[binding]`

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `type` | `"mcp"` \| `"prompted_llm"` \| `"http"` | oui | Détermine quelle sous-section `[binding.<type>]` est lue. |

### 2.6 `[binding.mcp]`

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `server` | string | oui | Adresse du serveur MCP. Forme `command:<cmd>` (stdio) ou `url:<http-url>` (HTTP/SSE). |
| `tool_name` | string | oui | Nom du tool MCP à invoquer. |
| `arg_mapping` | objet `{ mcp_arg = "${args.n3uron_arg}" }` | non | Mapping explicite si les noms diffèrent. Par défaut : passage transparent. |
| `result_mapping` | objet | non | Mapping de la sortie MCP vers le schéma `schema_out` N3UR0N. Par défaut : passage transparent. |
| `env` | objet `{ key = "value" }` | non | Variables d'env ajoutées au sous-processus (stdio uniquement). Peut référencer des secrets : `{{secret.name}}`. |

### 2.7 `[binding.prompted_llm]`

Binding de référence pour le cas "modèle généraliste + prompt de spécialisation".

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `model_endpoint` | string | oui | URL OpenAI-compatible. Ex : `http://localhost:11434` (Ollama). |
| `model` | string | oui | Nom de modèle côté upstream. Ex : `llama3.2:3b`. |
| `api_key` | string | non | Secret. Recommandé : `{{secret.openai_api_key}}`. |
| `system_prompt` | string (multiline) | oui | Le prompt de spécialisation. |
| `user_template` | string | non | Template du message utilisateur construit à partir des args. Ex : `"Traduire en anglais : {{args.text}}"`. Par défaut : sérialisation JSON des args. |
| `parameters` | objet | non | Paramètres LLM : `temperature`, `max_tokens`, `top_p`. |
| `output_parser` | `"text"` \| `"json"` | non | Comment interpréter la sortie. `"text"` (défaut) wrappe dans `{ text: "..." }`. `"json"` parse le `content` comme JSON et valide contre `schema_out`. |

### 2.8 `[binding.http]`

Binding pour forwarder vers un endpoint HTTP arbitraire.

| Champ | Type | Obligatoire | Description |
|---|---|---|---|
| `url` | string | oui | URL cible. Peut contenir des templates `{{args.x}}`. |
| `method` | `"GET"` \| `"POST"` \| `"PUT"` \| `"DELETE"` | oui | Méthode HTTP. |
| `headers` | objet | non | Headers ; valeurs peuvent référencer `{{secret.name}}`. |
| `body_template` | string ou objet | non | Pour `POST`/`PUT`. Si objet : sérialisation JSON après substitution `{{args.*}}`. Si string : envoyé tel quel. |
| `response_path` | string | non | JSONPath dans la réponse pour extraire le résultat. Ex : `"$.data.translation"`. Par défaut : body complet. |
| `timeout_ms` | int | non | Override du timeout pour ce binding. |

### 2.9 `[lifecycle]`

| Champ | Type | Défaut | Description |
|---|---|---|---|
| `warmup` | bool | `false` | Si `true`, le binding est instancié au boot (spawn MCP server, ping endpoint). Sinon lazy à la première invocation. |
| `timeout_ms` | int | `30000` | Timeout d'invocation. |
| `retry` | int | `0` | Nombre de retries automatiques sur erreur de transport (pas sur erreur applicative). |

---

## 3. Exemples complets

### 3.1 MCP wrapping d'un tool existant

```toml
[manifest]
version = "0.1"

[descriptor]
name = "github-search-issues"
description = "Cherche des issues GitHub par texte libre et filtre simple."
mode = "free"
tags = ["github", "search", "dev"]

[descriptor.schema_in]
type = "object"
required = ["query"]
properties = {
  query = { type = "string" },
  state = { type = "string", enum = ["open", "closed", "all"], default = "open" }
}

[descriptor.schema_out]
type = "object"
required = ["issues"]
properties = {
  issues = {
    type = "array",
    items = { type = "object" }
  }
}

[[descriptor.examples]]
intent = "trouver les issues ouvertes mentionnant 'memory leak' sur un projet"
args = { query = "memory leak repo:foo/bar", state = "open" }

disambiguation = """
Ne pas utiliser pour : créer ou commenter des issues (utiliser `github-issue-write`),
chercher dans un repo privé sans token configuré côté serveur MCP.
"""

[binding]
type = "mcp"

[binding.mcp]
server = "command:github-mcp-server --readonly"
tool_name = "search_issues"
env = { GITHUB_TOKEN = "{{secret.github_token}}" }
```

### 3.2 Prompted LLM (le cas le plus fréquent attendu)

```toml
[manifest]
version = "0.1"

[descriptor]
name = "legal-fr-summarizer"
description = "Résume un texte juridique français en 3-5 puces, sans interprétation."
mode = "free"
tags = ["legal", "summarization", "fr"]
lobe_ids = ["lobe.community.legal-fr.v1"]

[descriptor.schema_in]
type = "object"
required = ["text"]
properties = { text = { type = "string", minLength = 100 } }

[descriptor.schema_out]
type = "object"
required = ["summary_points"]
properties = {
  summary_points = { type = "array", items = { type = "string" } }
}

[[descriptor.examples]]
intent = "résumer un contrat de bail commercial"
args = { text = "..." }

disambiguation = """
Résume sans interpréter ni conseiller. Pour de l'analyse ou des recommandations,
préférer `legal-fr-analyst`. Pour des textes non-juridiques, préférer `summarizer-fr`.
"""

[binding]
type = "prompted_llm"

[binding.prompted_llm]
model_endpoint = "http://localhost:11434"
model = "qwen2.5:7b-instruct"
system_prompt = """
Tu es un assistant juridique français. Tu résumes des textes juridiques en français
de façon factuelle, sans interprétation ni recommandation, sous forme de 3 à 5 puces
courtes (≤ 25 mots chacune). Tu réponds STRICTEMENT en JSON :
{"summary_points": ["...", "...", "..."]}.
Aucun texte hors du JSON. Aucune interprétation. Si le texte fourni n'est pas juridique,
réponds {"summary_points": ["TEXTE NON JURIDIQUE"]}.
"""
user_template = "Texte à résumer :\n\n{{args.text}}"
parameters = { temperature = 0.0, max_tokens = 500 }
output_parser = "json"
```

### 3.3 HTTP forward vers une API externe

```toml
[manifest]
version = "0.1"

[descriptor]
name = "weather-now"
description = "Météo actuelle pour une ville donnée (open-meteo)."
mode = "free"
tags = ["weather", "geo"]

[descriptor.schema_in]
type = "object"
required = ["city"]
properties = { city = { type = "string" } }

[descriptor.schema_out]
type = "object"
required = ["temperature_c", "conditions"]
properties = {
  temperature_c = { type = "number" },
  conditions = { type = "string" }
}

[[descriptor.examples]]
intent = "quel temps fait-il à Lomé ?"
args = { city = "Lomé" }

[binding]
type = "http"

[binding.http]
url = "https://api.openweathermap.org/data/2.5/weather"
method = "GET"
headers = {}
# args sérialisés en query string par défaut sur GET
response_path = "$"
# (Note : open-meteo ne demande pas de clé ; OpenWeatherMap utilisée ici comme exemple
#  illustratif avec secret.)
```

---

## 4. Règles de validation au load

À chaque `(re)load` d'un cap.toml, n3uron effectue dans cet ordre :

1. **Parse TOML.** Erreur de syntaxe → manifeste rejeté, log d'erreur, cap absente du registre.
2. **Validation de version.** `manifest.version` ∈ {`"0.1"`}. Sinon rejet.
3. **Validation des champs obligatoires.** Manquant → rejet.
4. **Validation des schémas.** `schema_in` et `schema_out` doivent être des JSON Schema valides. Sinon rejet.
5. **Résolution des secrets référencés.** Chaque `{{secret.<name>}}` doit résoudre dans le store de secrets (cf. §5). Manquant → rejet avec message explicite.
6. **Probe du binding** *si* `lifecycle.warmup = true` :
   - MCP : tentative de connexion + appel `tools/list`, vérification que `tool_name` existe.
   - prompted_llm : appel `/v1/models` pour vérifier que `model` est servi.
   - http : `OPTIONS` ou `HEAD` selon support.
   Échec du probe → manifeste rejeté avec message diagnostic.
7. **Probe sémantique** *si* `descriptor.examples` non vide et flag `--validate-examples` activé : exécute chaque exemple, vérifie que la sortie valide `schema_out`. Échec → warning loggé, manifeste accepté (les exemples sont mis à jour plus souvent que le binding ne casse).

Si étape 1–6 passe, la cap est enregistrée dans `CapabilityRegistry` et exposée dans `describe_self`. Si une étape échoue, la cap n'est *pas* exposée et un message explicite est loggé (et visible dans `n3ur0n cap list`).

---

## 5. Gestion des secrets

Les secrets ne vivent **jamais** dans `cap.toml`. Ils sont référencés par nom logique : `{{secret.<name>}}`. Le store de secrets en v0.1 :

- **Source primaire** : keyring OS via `keyring-rs` (KeychainAccess macOS, Secret Service Linux, Credential Manager Windows). Le service est `n3ur0n`, le user est `<name>`.
- **Source secondaire (fallback)** : variable d'env `N3UR0N_SECRET_<NAME_UPPERCASE>`. Pratique pour Docker / CI.
- **Source tertiaire (dev uniquement)** : fichier `~/.n3ur0n/secrets.toml` permissions `0600`. Affiché avec warning au boot ("secrets in plain file — use keyring in production").

CLI d'administration : `n3ur0n secrets set <name>` (prompt sécurisé), `n3ur0n secrets list` (noms seulement, jamais les valeurs), `n3ur0n secrets unset <name>`.

Aucune commande ne *retourne* la valeur d'un secret. La résolution se fait uniquement au moment de l'invocation du binding, en mémoire, sans logging.

---

## 6. Templating

Le template `{{args.<path>}}` est utilisé dans :
- `binding.prompted_llm.user_template`
- `binding.http.url`, `binding.http.body_template` (si string)
- `binding.mcp.arg_mapping` (côté valeurs)

Le template `{{secret.<name>}}` est utilisé dans :
- `binding.http.headers`
- `binding.mcp.env`
- `binding.prompted_llm.api_key`

Règles :
- Substitution textuelle après résolution. Si la valeur est une string, insertion directe ; si c'est un objet/tableau/nombre, sérialisation JSON.
- Pas d'expression, pas de conditionnels, pas de boucles. Si besoin de logique, sortir vers un binding `mcp` qui peut implémenter la logique.
- Un template non résolu (path inexistant dans `args` ou nom de secret inconnu) provoque une erreur d'invocation, jamais une substitution silencieuse par chaîne vide.

---

## 7. Hot-reload

Le runtime watch le dossier des manifestes (`~/.n3ur0n/caps/` par défaut, override par `N3UR0N_CAPS_DIR`).

- **Création** d'un fichier `<name>.toml` → load + validation. Cap ajoutée au registre si valide.
- **Modification** → unload + reload. Pendant la fenêtre de bascule (typiquement quelques ms), une invocation en cours utilise l'ancienne version ; les nouvelles utilisent la nouvelle.
- **Suppression** → unload. Cap retirée du registre. Les peers distants verront la cap disparaître au prochain `describe_self`.

Le `CapabilityRegistry` notifie tous les peers connectés via un evènement `tools/list_changed` (si MCP est utilisé comme transport de gouvernance — sinon best-effort propagation lors du prochain `describe_self`).

Pour MCP en particulier : si le serveur MCP signale `notifications/tools/list_changed` pendant sa vie, n3uron re-list les tools mais **ne modifie pas les caps existantes** — celles-ci dépendent du `cap.toml`, pas du tool MCP. Si le tool référencé disparaît, la cap passe en état "binding broken" jusqu'à correction.

---

## 8. Questions ouvertes

À trancher avant gel du format.

### 8.1 Multi-cap par fichier ?

Position par défaut : non. Un fichier, une cap. Argument pour : lisibilité, version-control par cap, partage simple. Argument contre : un MCP server qui expose 20 tools → 20 fichiers presque identiques. Compromis possible : un fichier `_template.toml` partagé + 20 fichiers minimaux qui le `include`. Pas nécessaire en v0.1.

### 8.2 Inheritance vs override de description depuis MCP ?

Position par défaut : pas d'inheritance. Le `description` est canonique dans `cap.toml`. Argument pour : single source of truth, contrôle éditorial par publisher. Argument contre : friction pour wrapper rapide un MCP server tiers. Compromis : `n3ur0n mcp introspect` pré-remplit la description depuis MCP au scaffoldage ; le publisher est libre d'éditer.

### 8.3 Schémas : déclarés ou inhérités ?

Position défendue ici : déclarés dans `cap.toml`, validés à la conformité avec le binding au boot. Argument pour : drift détectable, comportement prévisible. Argument contre : duplication d'effort. Alternative : marquer `schema_in = "inherit"` qui demande à n3uron de pull depuis le binding. Pas tranché. Recommandation : v0.1 = déclaré obligatoire, v0.2 = ajouter `"inherit"` comme valeur spéciale si demande forte.

### 8.4 Faut-il un champ `version` côté `descriptor` ?

Pour que les peers puissent détecter "la cap A chez ce publisher a changé". Argument pour : permet caching côté consumer, anti-dérive. Argument contre : complexifie la spec, force discipline de versioning. Position par défaut : reporter en v0.2. En v0.1 le `describe_self_fetched_at` côté peer DB suffit comme proxy de fraîcheur.

### 8.5 Bindings composés / chaînés ?

Une cap qui est elle-même un plan figé sur plusieurs autres caps. Tentation forte de l'ajouter dès v0.1. Position : non. Tant que le planner peut composer des caps à la volée, un "binding composé" est redondant. À reconsidérer si le coût de planification devient problématique.

### 8.6 Format de manifeste hors-TOML ?

JSON et YAML possibles techniquement. TOML choisi pour : lisibilité dans le contexte de config (commentaires natifs, multiline strings, structure hiérarchique sans confusion). Pas d'engagement à supporter d'autres formats en v0.1.

---

## 9. Implications pour le runtime

Symboles touchés par cette spec (cf. annexe doc de recommandations §8).

| Crate / fichier | Changement |
|---|---|
| `crates/core/src/capability.rs` | `CapabilityDecl` reste, gagne les champs optionnels `examples`, `disambiguation`, `negative_examples` (déjà prévu dans reco §4.1). |
| `crates/node/src/registry.rs` | `CapabilityRegistry` charge depuis fichiers `cap.toml` au lieu d'être câblé à des `impl Backend`. |
| `crates/node/src/manifest.rs` *(nouveau)* | Parser + validateur du format. |
| `crates/node/src/bindings/mod.rs` *(nouveau)* | Trait `Binding` (équivalent du `Backend` actuel, mais générique sur le type de binding). |
| `crates/node/src/bindings/mcp.rs` *(nouveau)* | Client MCP — stdio + HTTP. |
| `crates/node/src/bindings/prompted_llm.rs` *(nouveau)* | Wrapper minimal sur `OpenAIBackend`, paramétré par prompt + modèle. |
| `crates/node/src/bindings/http.rs` *(nouveau)* | Forward HTTP générique. |
| `crates/adapters/src/*` | Devient code legacy à supprimer une fois la migration faite. |
| `crates/server/src/cli.rs` | Sous-commandes `cap` et `secrets`. |

---

## 10. Hors-périmètre explicite (v0.1)

- Bindings : WASM, subprocess générique non-MCP, gRPC, et composés.
- Templating riche (Jinja-like) — la substitution reste textuelle simple.
- Authentification d'invocation côté binding (au-delà de tokens dans les headers).
- Métriques / observabilité par cap (à venir avec OpenTelemetry, hors-scope v0.2).
- Versionning sémantique des caps (cf. §8.4).
- Marketplace de templates communautaires (chantier produit, pas spec).

---

*Fin du brouillon. Trois pages serrées, ~10 minutes de lecture. Vise à forcer les décisions §8 avant gel.*
