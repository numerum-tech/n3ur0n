# N3UR0N — Blob Protocol v0.1 (brouillon)

**Date** : 2026-05-12
**Statut** : brouillon de spec, non implémenté. À discuter avant gel.
**Objet** : définir comment des contenus binaires (fichiers, médias, documents générés) transitent entre instances n3uron sans violer les invariants protocolaires (envelopes JSON signées, request/response, pas de streaming).
**Portée** : transfert de blobs auxiliaire au verbe `invoke`. Ne touche pas aux 4 verbes existants. Ajoute un endpoint HTTPS sur le listener publisher.
**Lecteur** : implémenteur runtime, designer du format `cap.toml` / `backend.toml`, équipe UI Tauri.

---

## 0. Préambule

Le protocole N3UR0N v0.2 signe et canonicalise des envelopes JSON. Faire transiter un PDF de 5 MB en base64 dans le `payload` d'un message produit une envelope de ~7 MB à canonicaliser, signer, vérifier et router — désastreux opérationnellement et inutilement coûteux en CPU crypto.

Le présent document spécifie une *couche annexe* qui adresse les contenus binaires *par leur hash*, transporte les bytes sur le même listener HTTPS que les messages (mais via un endpoint séparé), et lie cryptographiquement chaque opération de blob à une cap invoquée. La signature ne couvre plus les MB de bytes — elle couvre des références constantes au hash. L'intégrité bout-en-bout est préservée par les hashes eux-mêmes.

Trois cas d'usage moteurs :
- **PDF → md / docx** : consumer pousse un PDF chez un publisher convert, reçoit en retour une référence vers le docx généré, télécharge.
- **JSON → docx** : consumer envoie du JSON structuré dans le payload classique, reçoit une référence vers le docx généré, télécharge. Pas d'upload nécessaire.
- **Image → caption** : consumer pousse une image, reçoit du texte (inline si court).

---

## 1. Invariants préservés

Cette spec **ne modifie pas** les invariants protocolaires de l'archi v0 §11 :

- Les 4 verbes (`describe_self`, `get_known_peers`, `ping`, `invoke`) restent inchangés.
- Les envelopes restent canonicalisées JCS et signées Ed25519.
- Pas de streaming protocolaire.
- Pas de session.
- Request/response uniquement.

Le protocole blob est *à côté* du protocole de messages, sur le même listener HTTPS, mais via un endpoint distinct et un format de tickets indépendant. Aucune envelope `invoke` n'est altérée par cette spec — seul le *contenu* du `payload` peut désormais référencer des blobs.

---

## 2. Concepts

### 2.1 Blob

Un *blob* est une séquence d'octets identifiée canoniquement par son hash SHA-256. Le hash est encodé `"sha256:" + lowercase_hex_64`. Forme canonique :

```
sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
```

Cette identification est *content-addressed* : deux blobs au contenu identique partagent leur identifiant peu importe leur origine.

### 2.2 BlobRef

Une *BlobRef* est l'objet JSON par lequel les payloads des envelopes référencent un blob. Schéma canonique :

```json
{
  "hash": "sha256:9f86...",
  "size": 524288,
  "mime": "application/pdf",
  "fetch_url": "https://publisher.example/n3ur0n/v0/blobs/sha256:9f86..."
}
```

- `hash` (obligatoire) : identifiant content-addressed.
- `size` (obligatoire) : taille en octets. Permet validation à l'arrivée et budget mémoire/disque.
- `mime` (obligatoire) : MIME type. Permet typage côté UI sans inspection du contenu.
- `fetch_url` (optionnelle) : URL canonique de récupération. Présente dans les BlobRef *sortants* (résultat d'un invoke côté publisher). Absente dans les BlobRef *entrants* (le publisher reçoit l'arg, il sait déjà chez qui chercher si nécessaire — généralement il a reçu le blob par upload juste avant).

### 2.3 Convention `x-n3uron-type: "blob"`

Dans un `schema_in` ou `schema_out`, un champ destiné à un blob est annoté :

```json
{
  "type": "object",
  "x-n3uron-type": "blob",
  "properties": {
    "hash": { "type": "string", "pattern": "^sha256:[a-f0-9]{64}$" },
    "size": { "type": "integer", "minimum": 1 },
    "mime": { "type": "string" },
    "fetch_url": { "type": "string", "format": "uri" }
  },
  "required": ["hash", "size", "mime"]
}
```

Le runtime n3uron reconnaît cette extension. Au moment de l'invocation :
- Pour un argument entrant marqué `x-n3uron-type: "blob"` non encore présent côté publisher, le runtime n3uron *côté consumer* déclenche l'upload du blob avant d'envoyer l'envelope `invoke`.
- Pour un résultat sortant marqué `x-n3uron-type: "blob"`, le runtime *côté consumer* récupère le blob via `fetch_url` après avoir reçu l'envelope de réponse.

---

## 3. Endpoint `/n3ur0n/v0/blobs`

Trois opérations HTTP sur le listener publisher (le même qui sert `/n3ur0n/v0/messages` et `/n3ur0n/v0/health`).

### 3.1 `PUT /n3ur0n/v0/blobs/:hash`

Upload d'un blob.

**Headers obligatoires** :
- `Content-Type: application/octet-stream` (le contenu lui-même).
- `X-N3UR0N-Ticket: <base64-encoded ticket envelope>` (autorisation signée, cf. §4).

**Comportement publisher** :
1. Vérifier le ticket (signature, expiration, opération `put`, hash référencé).
2. Vérifier que le hash du body reçu correspond exactement à `:hash`. Mismatch → `400 Hash Mismatch`.
3. Vérifier la taille reçue contre `ticket.size`. Mismatch → `400 Size Mismatch`.
4. Vérifier quota du `ticket.sender_id` (cf. §6). Dépassé → `429 Quota Exceeded`.
5. Vérifier que la cap référencée par le ticket existe et accepte des uploads de ce mime type. Sinon → `403 Capability Refuses Upload`.
6. Stocker le blob, indexé par hash.

**Réponse en succès** : `201 Created` avec body JSON minimal :
```json
{
  "hash": "sha256:...",
  "size": 524288,
  "expires_at": "2026-05-12T15:30:00Z"
}
```

`expires_at` annonce la TTL accordée (cf. §5).

**Idempotence** : si le hash existe déjà chez le publisher (mêmes bytes), `200 OK` retourné immédiatement sans relire le body. Le consumer SHOULD vérifier l'existence via `HEAD` avant de transférer (cf. §3.3).

### 3.2 `GET /n3ur0n/v0/blobs/:hash`

Téléchargement d'un blob.

**Headers obligatoires** :
- `X-N3UR0N-Ticket: <base64-encoded ticket envelope>` (autorisation de download).

**Comportement publisher** :
1. Vérifier le ticket (signature, expiration, opération `get`, hash référencé).
2. Vérifier que le `ticket.sender_id` a le droit de récupérer ce blob (cf. §4.3 sur l'autorisation).
3. Vérifier que le blob existe et n'est pas expiré.

**Réponse en succès** : `200 OK` avec :
- `Content-Type: <mime du blob>`.
- `Content-Length: <taille>`.
- `ETag: "<hash>"`.
- Body : les octets bruts.

**Résistance partielle aux ranges** : v0.1 ignore `Range:`. Téléchargement complet uniquement. Resume = à reconsidérer en v0.2 si retours utilisateurs.

### 3.3 `HEAD /n3ur0n/v0/blobs/:hash`

Vérification d'existence sans télécharger. Pas de ticket requis pour `HEAD` (l'information révélée est "ce hash existe-t-il" — minimal en termes de fuite). Sert à éviter les uploads redondants.

**Réponses possibles** :
- `200 OK` avec headers `Content-Length`, `Content-Type` : blob existe et fetchable (sous réserve de ticket pour `GET`).
- `404 Not Found` : blob absent.

### 3.4 `DELETE /n3ur0n/v0/blobs/:hash`

Suppression explicite par le propriétaire.

**Headers obligatoires** :
- `X-N3UR0N-Ticket: <base64>` avec opération `delete`.

**Comportement** :
1. Vérifier le ticket. L'opération `delete` n'est acceptée que si le `ticket.sender_id` correspond à l'identité qui a uploadé le blob.
2. Supprimer immédiatement (pas de GC déféré).
3. `204 No Content`.

Cas d'erreur : `403 Forbidden` si le sender_id ne matche pas l'uploader.

---

## 4. Format de ticket

Un *ticket* est une envelope N3UR0N régulière, mais avec un verbe étendu et un payload structuré qui autorise une opération de blob précise. Le format wire est identique à toute autre envelope (canonicalisée JCS, signée Ed25519), garantissant :

- Authentification de l'agissant (signature Ed25519).
- Intégrité (JCS + signature).
- Anti-replay (nonce + fenêtre timestamp ±5 min).
- Audit (les tickets peuvent être loggés comme tout message).

### 4.1 Structure

```json
{
  "sender_id": "n3:abc...",
  "recipient_id": "n3:def...",
  "timestamp": 1747058400,
  "nonce": "01HXYZ...",
  "verb": "blob_ticket",
  "payload": {
    "operation": "put" | "get" | "delete",
    "hash": "sha256:...",
    "size": 524288,
    "mime": "application/pdf",
    "capability": "convert_pdf",
    "expires_at": 1747058700,
    "purpose": "input" | "output" | "owned"
  },
  "sender_public_key": "...",
  "signature": "..."
}
```

### 4.2 Champs du payload

- `operation` (obligatoire) : verbe de l'opération.
- `hash` (obligatoire pour `put` et `get`) : hash du blob concerné.
- `size` (obligatoire pour `put`) : taille en octets, vérifiée à réception.
- `mime` (obligatoire pour `put`) : MIME déclaré.
- `capability` (obligatoire pour `put`) : nom de la cap pour laquelle le blob est destiné. Permet au publisher d'appliquer la politique de la cap (mode `restricted` exige whitelist, etc.).
- `expires_at` (obligatoire) : timestamp Unix au-delà duquel le ticket est invalide. Par convention ≤ 5 min après `timestamp`.
- `purpose` (obligatoire) : sémantique de l'opération.
  - `"input"` : blob qui sera passé en arg d'une invocation.
  - `"output"` : blob produit par une invocation, le consumer le télécharge.
  - `"owned"` : blob possédé par le sender (pour `delete`).

### 4.3 Règle d'autorisation

L'autorisation d'une opération suit la *règle de propriété transitive du ticket* :

- **`PUT`** : autorisé si le ticket est signé par un sender qui satisfait le mode d'accès de la cap référencée :
  - Cap mode `Public` (wire `"free"`) : tout sender.
  - Cap mode `Restricted` : sender dans la whitelist ou présentant un `subscription_token`.
  - Cap mode `Private` : refusé (les caps privées ne sont pas exposées au réseau, donc aucun upload externe ne devrait jamais arriver).
- **`GET`** : autorisé si le sender du ticket est *soit* l'uploader original (`put.sender == get.sender`), *soit* listé dans le `recipients_whitelist` du blob (sortie d'invocation : le publisher autorise le consumer cible à récupérer).
- **`DELETE`** : autorisé uniquement si `sender == uploader`.

### 4.4 Encodage on-the-wire

Le ticket est sérialisé en JCS, puis encodé en base64url *sans padding*, et placé dans le header HTTP `X-N3UR0N-Ticket`. Taille typique : ~600 octets après base64. Bien dans les limites HTTP/2.

---

## 5. Lifecycle des blobs

### 5.1 TTL par défaut

- **Blobs d'input** (purpose `input`) : TTL de 1 heure après dernier accès. Permet ré-invocation rapide de la même cap sur les mêmes données sans re-upload. Au-delà, GC.
- **Blobs d'output** (purpose `output`) : TTL de 24 heures après création. Donne au consumer le temps de télécharger même en cas de coupure réseau, sans monopoliser le disque publisher indéfiniment.
- **Blobs détenus** (purpose `owned`) : TTL de 1 heure depuis création, ou jusqu'au DELETE explicite.

TTL négociable : un ticket de `put` peut demander une TTL plus longue via `payload.requested_ttl_secs` (max 7 jours v0.1). Le publisher accepte ou refuse silencieusement — sa réponse `201 Created` contient le `expires_at` *effectivement* accordé, qui peut être inférieur à la demande.

### 5.2 Garbage collection

Job background côté publisher :
- Fréquence : toutes les 10 min.
- Critère : tout blob dont `expires_at < now()`.
- Action : suppression atomique du blob + entrée d'index.

Le GC est best-effort. Un blob expiré peut survivre quelques minutes au-delà de `expires_at` en pratique. Les opérations `GET` sur blob expiré retournent `404 Not Found` même si la suppression effective n'a pas encore eu lieu.

### 5.3 Cache consumer

Côté consumer, un cache blob local indexé par hash dans `~/.n3ur0n/blobs/sha256/<hash>` :
- Stratégie d'éviction : LRU bornée par quota disque (par défaut 1 GB, configurable).
- Persistant entre sessions n3uron.
- Vérifié au démarrage : tout fichier dont le hash recomputé ne matche pas son nom est supprimé.

Permet :
- Ré-upload sans recompute : si le user retraite le même PDF, le hash est déjà calculé.
- Téléchargement skippé : si un publisher renvoie un blob qu'on a déjà (rare mais possible avec content-addressing), le cache court-circuite le GET.

---

## 6. Quotas

### 6.1 Quotas publisher

Le publisher déclare ses quotas dans `backend.toml` (ou config globale) :

```toml
[blob_quotas]
default_per_peer_bytes = 104857600  # 100 MB par peer
default_per_peer_blobs = 50         # 50 blobs max par peer
total_disk_bytes = 10737418240      # 10 GB total
```

Application :
- À chaque `PUT`, le publisher vérifie `(somme des sizes des blobs actifs du sender_id) + size_nouveau <= default_per_peer_bytes`.
- Dépassement → `429 Quota Exceeded`.
- Quotas par-peer surchargeables par cap dans `cap.toml` :

```toml
[blob_quota]
per_peer_bytes = 524288000  # cette cap accepte plus
```

### 6.2 Quotas consumer

Le consumer applique ses propres limites pour prévenir le DOS de soi-même par un cap malveillant qui renverrait des outputs énormes :

```toml
[consumer.blob_limits]
max_download_size = 209715200       # refuse output > 200 MB
total_cache_bytes = 1073741824      # cache local 1 GB
```

Refus côté consumer = `413 Payload Too Large` côté response handler, avec log explicite. Le blob reste chez le publisher, qui le garbage-collectera selon sa TTL.

---

## 7. Intégration avec `cap.toml` / `backend.toml`

### 7.1 Déclaration de schéma avec blobs

Un cap qui prend un PDF et produit un docx :

```toml
[manifest]
version = "0.3"

[descriptor]
name = "pdf-to-docx"
description = "Convertit un PDF en document Word .docx avec conservation de la mise en page basique."
mode = "free"
version = "1.0.0"
languages = ["en", "fr"]
tags = ["conversion", "office", "pdf", "docx"]

[descriptor.schema_in]
type = "object"
required = ["input"]
properties = { input = {
  type = "object",
  "x-n3uron-type" = "blob",
  properties = {
    hash = { type = "string", pattern = "^sha256:[a-f0-9]{64}$" },
    size = { type = "integer", minimum = 1, maximum = 52428800 },  # 50 MB max
    mime = { type = "string", const = "application/pdf" }
  },
  required = ["hash", "size", "mime"]
}}

[descriptor.schema_out]
type = "object"
required = ["output"]
properties = { output = {
  type = "object",
  "x-n3uron-type" = "blob",
  properties = {
    hash = { type = "string" },
    size = { type = "integer" },
    mime = { type = "string", const = "application/vnd.openxmlformats-officedocument.wordprocessingml.document" },
    fetch_url = { type = "string", format = "uri" }
  },
  required = ["hash", "size", "mime", "fetch_url"]
}}

[[descriptor.examples]]
user_intent = "convertir ce PDF en Word"
args = { input = { hash = "sha256:...", size = 524288, mime = "application/pdf" } }
expected_output = { output = { hash = "sha256:...", size = 102400, mime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document" } }

[binding]
type = "mcp"
backend = "pdf-tools-mcp"
```

### 7.2 Déclaration de politique de données

Un nouveau bloc optionnel `[descriptor.data_policy]` permet au publisher de déclarer ses engagements :

```toml
[descriptor.data_policy]
retention = "ephemeral"          # ephemeral | retained | trained-on
post_processing_ttl_secs = 3600  # blobs purgés 1h après dernier usage
location_jurisdiction = "FR"     # juridiction de stockage déclarée
encryption_at_rest = "aes256"    # null | "aes256" | "..."
```

Engagement *contractuel* (le publisher peut mentir), pas garantie technique. La déclaration est cependant signée à chaque `describe_self`, donc auditable. Un peer qui change sa policy sans bump de version du cap est techniquement détectable.

Recommandation : le planner *priorise les caps avec policy déclarée stricte* pour les workflows sensibles. Un user peut configurer son planner avec `prefer_data_policy = "ephemeral"` ou refuser absolument certains modes.

### 7.3 Préférence locale pour fichiers sensibles

Le planner doit appliquer une règle générale : *pour toute cap dont le `schema_in` contient un blob, préférer une cap locale (peer_id == self) à une cap distante à qualité équivalente*. Cette règle est inscrite au niveau du retrieval/ranking, pas du protocole.

---

## 8. Inlining : seuil et règles

Les BlobRef ne sont pas obligatoires pour tout fichier — pour les très petits payloads binaires, l'inlining base64 reste préférable.

### 8.1 Seuil

- Seuil dur : **256 KB après base64** (≈ 192 KB binaire). Au-dessous, base64 inline accepté.
- Au-dessus, le runtime *côté consumer* refuse l'inlining et force le passage par blob — même si l'auteur du cap.toml a écrit un schéma `{type: "string", contentEncoding: "base64"}`.

Justification du seuil : 256 KB d'envelope JSON canonicalise et signe en ~1 ms sur matériel typique. Au-delà, la latence devient perceptible et la consommation mémoire grimpe.

### 8.2 Cas où l'inlining est légitime

- Miniatures d'image (<100 KB).
- Petits PDF (rare, mais factures simples).
- Réponses textuelles longues (md, sql, json structurés).
- Snippets audio courts (<5 s).

### 8.3 Cas où le blob est obligatoire

- Tout document de bureau (.pdf, .docx, .xlsx, .pptx) — typiquement > 256 KB.
- Toute image full-res.
- Tout fichier audio/vidéo de plus de quelques secondes.

---

## 9. Sécurité : ce qui est garanti et ce qui ne l'est pas

### 9.1 Garanti

- **Intégrité bout-en-bout** : un blob reçu hashé à H prouvé est nécessairement le blob référencé dans l'envelope signée. Toute altération en transit ou à l'arrivée provoque mismatch et rejet.
- **Authentification d'opération** : tout upload, download, delete est cryptographiquement lié à une identité Ed25519. Pas d'opérations anonymes.
- **Confidentialité en transit** : TLS (rustls, certs auto-signés TOFU pour cohérence avec le reste de la stack).
- **Résistance au rejeu** : les tickets ont nonces et timestamps, mêmes mécanismes anti-replay que les messages.
- **Liaison à la cap** : le ticket déclare la cap pour laquelle le blob est destiné. Le publisher peut refuser un blob qui prétendrait servir une cap qu'il n'expose pas.

### 9.2 Non garanti

- **Confidentialité du contenu vis-à-vis du publisher**. Une cap qui traite un PDF voit son contenu en clair, par construction de la fonction de la cap. Aucune crypto de protocole ne peut empêcher ça. Compensation : `data_policy` déclarative + préférence locale + lobes curés.
- **Disponibilité long terme**. Les blobs sont éphémères (TTL bornée). Pas de stockage pérenne dans ce protocole. Si besoin, ajouter une cap `archive_store` qui assume cette responsabilité explicitement.
- **Protection contre publisher malveillant qui collecte**. Un publisher peut logger tous les inputs reçus, peu importe sa `data_policy` déclarée. Le contrat est social/juridique, pas technique.

### 9.3 Surfaces d'attaque connues et mitigations

- **DoS par flood d'uploads** : mitigé par quotas par-peer.
- **Espace disque épuisé** : mitigé par quota total + GC agressif sur TTL.
- **Hash collisions** : SHA-256 considéré sûr en 2026. Si compromission future, prévoir migration vers SHA-3 ou BLAKE3 via préfixe dans le hash literal (`"blake3:..."` etc.).
- **Tentative de fetch de blobs d'autres consumers** : mitigé par règle d'autorisation §4.3 (seul l'uploader ou le destinataire whitelisté peut GET).
- **Fuite d'information par existence de hash** (`HEAD` sans ticket révèle si un hash existe) : risque de side-channel mineur. Atténuation possible : exiger un ticket même sur `HEAD`. Tranchage en §11 (question ouverte).

---

## 10. UX et runtime — implications côté Tauri

### 10.1 File picker → BlobRef

Quand l'utilisateur sélectionne un fichier via le picker Tauri natif :
1. Le shell desktop reçoit un chemin local.
2. Le backend calcule SHA-256 en streaming (sans charger tout en mémoire).
3. Le fichier est copié/hardlinké dans `~/.n3ur0n/blobs/sha256/<hash>`.
4. Une BlobRef `{hash, size, mime}` est construite et passée au planner / form / direct invoke comme arg.

Le contenu *n'entre jamais dans le contexte LLM*. Le planner manipule des références, pas des MB de base64.

### 10.2 Téléchargement → ouverture native

Quand une cap renvoie un BlobRef en sortie :
1. Le runtime parse la `fetch_url`, forge un ticket `GET`, télécharge.
2. Vérifie le hash à réception. Mismatch → erreur affichée, fichier non sauvé.
3. Sauve sous `~/.n3ur0n/blobs/sha256/<hash>` (cache) ET propose à l'utilisateur un emplacement de destination via dialog Tauri.
4. Optionnellement, lance l'application système associée au mime (`open` macOS, `xdg-open` Linux, `ShellExecute` Windows).

### 10.3 Progress UX

Un upload de 50 MB sur réseau lent prend ~1 min. Le shell Tauri affiche une barre de progression construite à partir de :
- Côté upload : bytes envoyés / total (callback sur le PUT request).
- Côté download : `Content-Length` + bytes reçus.

Pas de progression *protocolaire* (pas de streaming) ; c'est de la cosmétique HTTP côté client.

### 10.4 Gestion des fichiers expirés

Un fichier dans le cache local dont le blob distant a expiré est marqué "stale" mais conservé localement (le user peut l'avoir sauvé ailleurs). Le runtime ne re-fetch pas automatiquement, mais propose un re-upload si la même cap est invoquée à nouveau avec ce hash.

---

## 11. Questions ouvertes

À trancher avant gel de la spec.

### 11.1 Ticket obligatoire sur `HEAD` ?

Position par défaut : non. `HEAD` révèle uniquement "ce hash existe-t-il chez ce publisher", fuite mineure. Imposer un ticket complique l'usage (chaque check d'existence = signature Ed25519). Argument contre : un attaquant peut sonder l'existence d'un hash spécifique (vérifier si un document précis a été uploadé). Tranchage : pour v0.1 laisser libre, monitorer les usages.

### 11.2 Resume / Range requests ?

Position par défaut : non en v0.1. Pour les uploads > 50 MB sur connexions instables, c'est un manque. Réponse : tolérable en v0.1 (le client retentera depuis zéro), à reconsidérer si retours utilisateurs négatifs en v0.2.

### 11.3 Compression côté serveur ?

Un publisher peut-il compresser un blob (gzip, zstd) avant stockage et le decompresser au `GET` ? Position par défaut : non. La sémantique content-addressed exige que les octets retournés au `GET` soient *exactement* ceux du hash. Toute transformation invaliderait le hash. Si compression désirée, c'est à l'auteur de la cap de produire un blob déjà compressé (`mime: "application/gzip"` etc.) et au consumer de décompresser.

### 11.4 Multi-blob dans un seul ticket ?

Pour un cap qui prend 10 PDF d'un coup, fait-il 10 tickets + 10 PUT, ou 1 ticket couvrant 10 hashes ? Position par défaut : 1 ticket par opération. Plus simple, plus inspectable, plus auditable. Pas d'optimisation prématurée pour le cas batch.

### 11.5 Encryption at rest, end-to-end ?

Hors-scope v0.1 (cf. §9). Pour des cas réels de documents sensibles (RH, juridique, médical), un mécanisme d'encryption asymétrique avec clé de session échangée à l'invoke serait viable mais ouvre une boîte de Pandore. À traiter dans une spec dédiée, post-v0.2.

### 11.6 Caps "purement-local-aware" ?

Une cap `binding.type = "local"` (process local) qui prend un blob — doit-elle quand même passer par le protocole de blob, ou peut-elle accepter un chemin local directement ? Position par défaut : passe quand même par le hash + cache local, pour cohérence. Le coût est marginal (le blob est déjà dans `~/.n3ur0n/blobs/` après file picker) et la uniformité simplifie le runtime.

---

## 12. Implications pour le runtime — symboles touchés

| Crate / fichier | Changement |
|---|---|
| `crates/core/src/blob.rs` *(nouveau)* | Types `BlobRef`, `BlobTicketPayload`, fonctions de validation. |
| `crates/core/src/message.rs` | Étendre `ProtocolVerb` pour inclure `BlobTicket` (verbe interne, jamais routé via `/messages`). |
| `crates/storage/src/blobs.rs` *(nouveau)* | Repo blobs côté publisher : index hash → métadonnées + chemin disque. |
| `crates/storage/migrations/` | Nouvelle migration : table `blobs` (hash, size, mime, uploader_id, capability, expires_at, recipients_whitelist). |
| `crates/server/src/http.rs` | Routes `PUT/GET/HEAD/DELETE /n3ur0n/v0/blobs/:hash`. |
| `crates/server/src/blob_gc.rs` *(nouveau)* | Job background de GC, tick toutes les 10 min. |
| `crates/node/src/client.rs` | Helpers `upload_blob(endpoint, blob_ref, bytes)`, `download_blob(blob_ref) -> Vec<u8>`. |
| `crates/node/src/planner/plan.rs` | Avant exécution d'un step, résoudre les BlobRef entrants (upload si nécessaire). |
| `crates/node/src/manifest/types.rs` | Ajout du bloc `[descriptor.data_policy]`, du bloc `[blob_quota]`. |
| `crates/server/ui/` | Composants UX : progress upload/download, file picker → BlobRef, propose-save-as. |

Effort total estimé : 2-3 semaines pour la couche core + endpoint + GC + intégration planner. UX Tauri : +1 semaine.

---

## 13. Hors-périmètre explicite (v0.1)

- **Streaming protocolaire**. Inscrit comme invariant ; reste interdit. Progression UX = cosmétique HTTP, pas protocole.
- **Encryption end-to-end** des blobs. Hors-scope. Cf. §11.5.
- **Stockage long terme / archive**. Pas la responsabilité du protocole de blob. Si besoin, ouvrir une cap dédiée `archive_store`.
- **Réplication / fanout / mirrors**. Un blob existe chez son publisher ; pas d'autre copie via le protocole. Replication entre publishers = chantier produit séparé.
- **CDN / edge caching**. Idem.
- **Authentification autre qu'Ed25519** sur les tickets. Cohérence avec le reste du protocole.
- **Métadonnées riches** (EXIF, tags, etc.) — les blobs sont des sacs d'octets opaques au protocole.

---

## 14. Cohérence avec le reste de la stack

### 14.1 Cap `AccessMode` ternaire

La spec respecte le ternaire `Private | Restricted | Public` (wire `"free"`) introduit en cours d'implémentation v0.2. Conséquences :

- Cap `Private` : ne participe jamais au blob protocol entrant. Un PUT pour une cap Private est `403`.
- Cap `Restricted` : upload accepté seulement si sender en whitelist ou subscription_token. La logique d'auth est identique à celle déjà appliquée à l'invoke.
- Cap `Public` (`free`) : upload accepté pour tout sender bien signé (sous quota).

### 14.2 Manifeste v0.3

Le format actuel `cap.toml` + `backend.toml` (manifeste v0.3) accommode les ajouts ici sans changement structurel : nouveaux blocs optionnels `[descriptor.data_policy]` et `[blob_quota]`, convention `x-n3uron-type: "blob"` dans les schémas JSON Schema déjà présents.

### 14.3 Versionning de cap (v0.2 — `version` semver)

Si une cap change sa `schema_in` pour passer d'un inline base64 à un BlobRef, c'est un *breaking change* MAJOR du `cap.version` (1.0.0 → 2.0.0). Les consumers détectent et invalident leurs caches de catalogue.

---

*Fin du brouillon. ~12 pages, 25 min de lecture. Vise à forcer les décisions §11 avant gel.*
