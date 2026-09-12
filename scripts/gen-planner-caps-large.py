#!/usr/bin/env python3
"""Generate the large adversarial capability corpus for the planner eval.

Why this exists
---------------
`planner_caps.json` holds 13 capabilities. The runtime bound is
`REMOTE_TOP_K = 20`, so the retrieval filter never drops anything and
`recall@K` is trivially 100%. At that size the suite cannot tell whether the
retriever works — it only ever measures the planner.

This corpus makes the filter bite. It is NOT random noise: random capabilities
are easy negatives and would produce a flattering number that says nothing.
Every distractor is written to sit in the *semantic neighbourhood* of a real
capability, reusing its vocabulary while being the wrong answer.

Three pressures, deliberately:

  - near-miss      : `transliterate`, `detect_language`, `back_translate` all
                     talk about languages and text, and none of them is
                     `translate`. Keyword overlap cannot separate them.
  - cross-lingual  : a share of the corpus is described in French. A lexical
                     index is blind to them; a multilingual embedder is not.
                     None of them is ever the expected answer, so a retriever
                     that over-weights them loses recall.
  - volume         : unrelated domains pad the catalogue to a realistic size
                     without being hard. They measure the bound, not the
                     ranking.

Output is deterministic — no RNG — so two measurements are comparable. Edit the
tables below and regenerate; never hand-edit the JSON.

Usage: python3 scripts/gen-planner-caps-large.py
"""

import json
import pathlib

# Synthetic peers the noise is spread over. Ids follow the `n3:` + base32
# shape; they are never dialled, only ranked.
PEERS = {f"noise{i:02d}": f"n3:evalnoise{i:02d}00000000000000000000"[:35] for i in range(1, 13)}

# ---------------------------------------------------------------------------
# Semantic neighbourhoods — the substance of this corpus.
#
# Each entry is (name, description, tags, one user intent). They are grouped by
# the real capability they crowd, which is what makes them adversarial: the
# group header names the capability that MUST still win.
# ---------------------------------------------------------------------------

NEIGHBOURHOODS = {
    "translate": [
        ("transliterate", "Convert text from one script to another (Cyrillic to Latin, Arabic to Latin) without translating the meaning.", ["language", "script"], "Write this Russian name in the Latin alphabet."),
        ("detect_language", "Identify which natural language a piece of text is written in, with a confidence score.", ["language", "detect"], "What language is this sentence in?"),
        ("romanize", "Romanize Japanese, Korean or Chinese text into Latin characters following a standard scheme.", ["language", "script"], "Give me the romaji for this Japanese sentence."),
        ("translate_subtitles", "Translate a subtitle file, preserving timing cues and line breaks.", ["language", "subtitles", "video"], "Translate this SRT subtitle file into Spanish."),
        ("localize_ui_strings", "Translate an application's UI string catalogue, keeping placeholders and pluralisation rules intact.", ["language", "i18n", "software"], "Localise these interface strings into German."),
        ("glossary_lookup", "Look a term up in a bilingual domain glossary and return approved translations.", ["language", "terminology"], "What is the approved French term for this legal expression?"),
        ("back_translate", "Translate a text into a pivot language and back, to expose meaning drift.", ["language", "quality"], "Round-trip this paragraph through Japanese to check the wording."),
        ("translate_html", "Translate an HTML document while leaving tags, attributes and scripts untouched.", ["language", "html"], "Translate this web page's text but keep the markup."),
        ("language_pair_support", "Report which source and target language pairs a translation engine supports.", ["language", "capability"], "Do you support Ewe to French?"),
    ],
    "summarize": [
        ("summarize_meeting", "Turn a meeting transcript into minutes with decisions and action items.", ["summary", "meeting"], "Write up the minutes of this meeting recording."),
        ("headline_generate", "Write a single short headline capturing an article's angle.", ["summary", "editorial"], "Give this article a headline."),
        ("bullet_digest", "Reduce a document to a bulleted digest of at most ten points.", ["summary", "bullets"], "Give me the bullet points of this report."),
        ("tldr_thread", "Summarise a long discussion thread into one paragraph per participant position.", ["summary", "discussion"], "TL;DR this whole thread."),
        ("extractive_sentences", "Select the most representative sentences verbatim, without rewriting.", ["summary", "extractive"], "Pull out the key sentences from this text, unchanged."),
        ("compress_prompt", "Shorten a prompt while preserving its instructions, for context-limited models.", ["summary", "prompt"], "Make this prompt shorter without losing instructions."),
        ("chapter_outline", "Produce a hierarchical outline of a long document by section.", ["summary", "outline"], "Give me the outline of this book chapter."),
        ("executive_brief", "Write a one-page executive brief with risks and recommendations.", ["summary", "business"], "Turn this study into an executive brief."),
        ("summarize_diff", "Describe what a code diff changes, in prose.", ["summary", "code"], "Explain what this pull request changes."),
        ("condense_transcript", "Condense an interview transcript, dropping filler and repetitions.", ["summary", "transcript"], "Condense this interview transcript."),
    ],
    "extract_keywords": [
        ("named_entities", "Extract people, organisations, locations and dates as typed entities.", ["extraction", "ner"], "List the people and companies mentioned here."),
        ("topic_model", "Cluster a corpus into latent topics with representative terms per topic.", ["extraction", "topics"], "What topics run through this set of documents?"),
        ("tag_suggest", "Suggest publication tags for a piece of content from a controlled vocabulary.", ["extraction", "tags"], "Suggest tags for this blog post."),
        ("extract_dates", "Find every date and time expression and normalise it to ISO 8601.", ["extraction", "dates"], "Pull out all the dates in this contract."),
        ("extract_emails", "Find email addresses and phone numbers in unstructured text.", ["extraction", "contact"], "Get the contact details out of this signature block."),
        ("taxonomy_classify", "Assign a document to one or more nodes of a predefined taxonomy.", ["extraction", "classification"], "Classify this document in our taxonomy."),
        ("keyphrase_rank", "Rank multi-word key phrases by salience rather than single terms.", ["extraction", "keyphrase"], "What are the key phrases of this paper?"),
    ],
    "web_search": [
        ("news_search", "Search recent news articles, filtered by date range and outlet.", ["search", "news"], "Find news from last week about this company."),
        ("image_search", "Search for images matching a description and return their URLs.", ["search", "images"], "Find pictures of this monument."),
        ("academic_search", "Search scholarly literature and return papers with citation counts.", ["search", "academic"], "Find papers on this subject."),
        ("site_search", "Search within one specific website's indexed pages.", ["search", "site"], "Search this documentation site for that function."),
        ("search_trends", "Report how search interest in a term evolved over time.", ["search", "trends"], "Is interest in this topic rising?"),
        ("patent_search", "Search patent databases by claim text and classification.", ["search", "patents"], "Find patents covering this mechanism."),
        ("autocomplete_suggest", "Return the query completions a search engine would propose.", ["search", "suggest"], "What do people search for after typing this?"),
    ],
    "fetch_url": [
        ("screenshot_url", "Render a URL in a headless browser and return a PNG screenshot.", ["web", "render"], "Take a screenshot of this page."),
        ("url_metadata", "Return a URL's Open Graph and meta tags without its body text.", ["web", "metadata"], "What are the meta tags of this page?"),
        ("check_link_status", "Check whether a list of URLs still resolve, reporting redirects.", ["web", "health"], "Are these links still alive?"),
        ("extract_main_article", "Strip navigation and ads from a page, returning only the article body.", ["web", "readability"], "Give me just the article text from this page."),
        ("rss_fetch", "Fetch and parse an RSS or Atom feed into structured entries.", ["web", "feed"], "Get the latest entries from this feed."),
        ("archive_snapshot", "Retrieve an archived snapshot of a URL at a past date.", ["web", "archive"], "What did this page look like last year?"),
        ("download_document", "Download a linked document and report its media type and size.", ["web", "download"], "Download the PDF behind this link."),
    ],
    "convert_currency": [
        ("unit_convert", "Convert between physical units of length, mass, volume or temperature.", ["conversion", "units"], "How many kilometres is 12 miles?"),
        ("timezone_convert", "Convert a timestamp from one time zone to another.", ["conversion", "time"], "What is 3pm Paris time in Tokyo?"),
        ("number_base_convert", "Convert an integer between binary, octal, decimal and hexadecimal.", ["conversion", "numbers"], "What is 255 in hexadecimal?"),
        ("historical_fx_rate", "Return the exchange rate between two currencies on a past date.", ["conversion", "finance", "history"], "What was the euro-dollar rate in March 2020?"),
        ("crypto_price", "Return the current price of a crypto asset in a fiat currency.", ["finance", "crypto"], "What is bitcoin worth right now?"),
        ("vat_calculate", "Compute value-added tax for an amount given a jurisdiction.", ["finance", "tax"], "What is the VAT on this amount in France?"),
        ("format_money", "Format a monetary amount according to a locale's conventions.", ["finance", "format"], "Format this amount the French way."),
    ],
    "weather_forecast": [
        ("air_quality", "Current air quality index and pollutant breakdown for a location.", ["environment", "air"], "How is the air quality in this city?"),
        ("marine_forecast", "Sea state, wave height and wind forecast for a coastal area.", ["weather", "marine"], "What is the sea like off this coast tomorrow?"),
        ("pollen_index", "Pollen concentration and allergy risk for a location.", ["environment", "health"], "Is the pollen bad there today?"),
        ("uv_index", "Ultraviolet index and recommended exposure limits.", ["environment", "sun"], "How strong is the sun there right now?"),
        ("historical_weather", "Observed weather for a location on a past date.", ["weather", "history"], "What was the weather there last Christmas?"),
        ("storm_alerts", "Active severe weather warnings issued for a region.", ["weather", "alerts"], "Are there any storm warnings for that region?"),
        ("sunrise_sunset", "Sunrise, sunset and civil twilight times for a location and date.", ["astronomy", "time"], "What time does the sun set there?"),
    ],
    "chat": [
        ("code_assistant", "Write, explain or debug source code in a named programming language.", ["llm", "code"], "Why does this function throw an exception?"),
        ("rewrite_tone", "Rewrite a text in a different register, keeping its content.", ["llm", "writing"], "Make this email sound more formal."),
        ("grammar_check", "Correct spelling, grammar and agreement without changing the style.", ["llm", "proofreading"], "Fix the mistakes in this paragraph."),
        ("brainstorm_ideas", "Produce a divergent list of ideas around a theme.", ["llm", "ideation"], "Give me ideas for a product launch."),
        ("sentiment_analysis", "Classify a text's sentiment as positive, negative or neutral with a score.", ["llm", "classification"], "Is this customer review positive or negative?"),
        ("classify_intent", "Map a user utterance to one of a fixed set of intents.", ["llm", "classification"], "Which intent does this support ticket belong to?"),
        ("answer_from_docs", "Answer a question using only a supplied document set, citing passages.", ["llm", "rag"], "Answer this using only the attached handbook."),
        ("roleplay_persona", "Hold a conversation in a defined persona and register.", ["llm", "persona"], "Reply as a medieval chronicler would."),
    ],
    "time": [
        ("date_diff", "Compute the elapsed time between two dates in chosen units.", ["time", "arithmetic"], "How many days between these two dates?"),
        ("format_date", "Format a timestamp according to a locale and pattern.", ["time", "format"], "Write this date the British way."),
        ("cron_next_run", "Compute the next execution times of a cron expression.", ["time", "scheduling"], "When does this cron expression next fire?"),
        ("business_days", "Count working days between two dates for a given country's calendar.", ["time", "calendar"], "How many working days are left this month here?"),
        ("epoch_convert", "Convert between Unix epoch seconds and a human-readable date.", ["time", "conversion"], "What date is this epoch timestamp?"),
        ("timezone_list", "List IANA time zones and their current UTC offsets.", ["time", "zones"], "Which time zone is that city in?"),
    ],
    "random_int": [
        ("random_uuid", "Generate a version 4 UUID.", ["random", "identifier"], "Give me a UUID."),
        ("random_choice", "Pick one element uniformly at random from a supplied list.", ["random", "selection"], "Pick one of these options for me."),
        ("shuffle_list", "Return a supplied list in random order.", ["random", "ordering"], "Shuffle these names."),
        ("dice_roll", "Roll dice in standard notation and return the total and each die.", ["random", "games"], "Roll two six-sided dice."),
        ("random_password", "Generate a password of a given length and character policy.", ["random", "security"], "Generate a strong password."),
        ("random_float", "Draw a random real number from a uniform or normal distribution.", ["random", "statistics"], "Give me a random number between zero and one."),
    ],
    "reverse": [
        ("reverse_words", "Reverse the order of words in a sentence, keeping each word intact.", ["string", "order"], "Put the words of this sentence in reverse order."),
        ("palindrome_check", "Report whether a string reads the same backwards, ignoring case and punctuation.", ["string", "test"], "Is this word a palindrome?"),
        ("rot13", "Apply the ROT13 substitution cipher to a string.", ["string", "cipher"], "Encode this with ROT13."),
        ("string_replace", "Replace every occurrence of a pattern in a string.", ["string", "edit"], "Replace all the commas here with semicolons."),
        ("slugify", "Turn a title into a URL-safe slug.", ["string", "url"], "Make a URL slug out of this title."),
        ("split_string", "Split a string on a separator and return the parts.", ["string", "split"], "Split this line on the tabs."),
        ("to_uppercase", "Convert a string to upper case, locale-aware.", ["string", "case"], "Put this in capitals."),
    ],
    "string_length": [
        ("word_count", "Count the words in a text.", ["text", "count"], "How many words is this?"),
        ("token_count", "Count how many model tokens a text occupies for a given tokeniser.", ["text", "tokens"], "How many tokens will this prompt cost?"),
        ("byte_size", "Report a string's size in bytes under a given encoding.", ["text", "encoding"], "How many bytes is this in UTF-8?"),
        ("line_count", "Count lines, optionally ignoring blank ones.", ["text", "count"], "How many lines does this file have?"),
        ("readability_score", "Compute readability indices such as Flesch-Kincaid.", ["text", "readability"], "How hard is this text to read?"),
        ("char_frequency", "Return the frequency of each character in a string.", ["text", "statistics"], "Which letter appears most in this text?"),
        ("sentence_count", "Count sentences, handling abbreviations.", ["text", "count"], "How many sentences are in this paragraph?"),
    ],
}

# French-described capabilities. None is ever an expected answer: they exist so
# a French query has plausible-looking wrong matches, and so a lexical index
# built on an English catalogue is measurably blind to half the corpus.
FRENCH = [
    ("resumer_document", "Résume un document long en conservant les points principaux et la structure.", ["résumé", "document"], "Fais-moi un résumé de ce document."),
    ("traduire_texte", "Traduit un texte vers une langue cible en respectant le registre.", ["traduction", "langue"], "Traduis ce texte en anglais."),
    ("compter_caracteres", "Compte les caractères, mots et lignes d'un texte.", ["texte", "comptage"], "Combien de caractères dans cette phrase ?"),
    ("heure_actuelle", "Donne l'heure courante dans un fuseau horaire donné.", ["temps", "fuseau"], "Quelle heure est-il à Lomé ?"),
    ("meteo_ville", "Prévisions météorologiques pour une ville sur plusieurs jours.", ["météo", "ville"], "Quel temps fera-t-il demain à Dakar ?"),
    ("convertir_devise", "Convertit un montant d'une devise vers une autre au taux du jour.", ["finance", "devise"], "Convertis 100 euros en francs CFA."),
    ("rechercher_web", "Recherche sur le web public et renvoie des résultats classés.", ["recherche", "web"], "Cherche des articles sur ce sujet."),
    ("extraire_mots_cles", "Extrait les termes saillants d'un texte sous forme de liste classée.", ["extraction", "mots-clés"], "Quels sont les mots-clés de ce texte ?"),
    ("inverser_chaine", "Inverse l'ordre des caractères d'une chaîne.", ["chaîne", "ordre"], "Inverse cette chaîne de caractères."),
    ("nombre_aleatoire", "Tire un entier aléatoire dans un intervalle donné.", ["aléatoire", "nombre"], "Donne-moi un nombre au hasard entre 1 et 100."),
    ("analyser_sentiment", "Analyse la tonalité d'un texte et renvoie un score.", ["analyse", "sentiment"], "Cet avis client est-il positif ?"),
    ("corriger_orthographe", "Corrige l'orthographe et la grammaire sans changer le style.", ["correction", "langue"], "Corrige les fautes de ce paragraphe."),
    ("generer_slug", "Transforme un titre en identifiant d'URL.", ["url", "chaîne"], "Fais un slug à partir de ce titre."),
    ("fuseaux_horaires", "Liste les fuseaux horaires et leur décalage courant.", ["temps", "fuseau"], "Dans quel fuseau se trouve cette ville ?"),
    ("qualite_air", "Indice de qualité de l'air et polluants pour une localité.", ["environnement", "air"], "La qualité de l'air est-elle bonne là-bas ?"),
]

# Unrelated domains. Easy negatives: they measure the bound, not the ranking.
FILLER_DOMAINS = {
    "devops": ["deploy_service", "rollback_release", "scale_replicas", "tail_logs", "restart_pod", "check_certificate", "rotate_secret", "run_migration"],
    "finance": ["invoice_create", "expense_categorise", "payroll_run", "ledger_reconcile", "budget_forecast", "tax_bracket", "loan_schedule"],
    "health": ["bmi_compute", "dose_convert", "symptom_triage", "appointment_slots", "vaccine_schedule", "calorie_lookup"],
    "legal": ["clause_compare", "jurisdiction_lookup", "deadline_compute", "citation_format", "redline_diff", "entity_register"],
    "geo": ["geocode_address", "reverse_geocode", "route_distance", "timezone_at_point", "elevation_at_point", "bounding_box"],
    "media": ["transcode_video", "extract_audio", "resize_image", "strip_exif", "waveform_render", "thumbnail_grid"],
    "iot": ["read_sensor", "set_thermostat", "device_inventory", "firmware_version", "power_usage"],
    "education": ["quiz_generate", "grade_answer", "curriculum_align", "flashcard_build", "reading_level"],
}


def cap(name, description, tags, intent, peer_idx):
    """One capability declaration, complete enough to be ranked fairly.

    `searchable_text` indexes name, description, tags, disambiguation,
    output_semantic and example intents — so a distractor missing any of them
    would be handicapped, and the measurement would flatter the retriever.
    """
    return {
        "peer": f"noise{(peer_idx % len(PEERS)) + 1:02d}",
        "decl": {
            "name": name,
            "description": description,
            "schema_in": {
                "type": "object",
                "required": ["input"],
                "properties": {"input": {"type": "string"}},
            },
            "schema_out": {
                "type": "object",
                "properties": {"result": {"type": "string"}},
            },
            "mode": "free",
            "tags": tags,
            "examples": [
                {
                    "user_intent": intent,
                    "args": {"input": "..."},
                    "expected_output": {"result": "..."},
                }
            ],
            "disambiguation": f"Use only for: {description.rstrip('.').lower()}.",
            "output_semantic": f"The result of {name.replace('_', ' ')}.",
        },
    }


def main():
    caps = []
    i = 0
    for group, entries in NEIGHBOURHOODS.items():
        for name, desc, tags, intent in entries:
            caps.append(cap(name, desc, tags, intent, i))
            i += 1
    for name, desc, tags, intent in FRENCH:
        caps.append(cap(name, desc, tags, intent, i))
        i += 1
    for domain, names in FILLER_DOMAINS.items():
        for name in names:
            caps.append(
                cap(
                    name,
                    f"{name.replace('_', ' ').capitalize()} — {domain} operation.",
                    [domain],
                    f"Please {name.replace('_', ' ')}.",
                    i,
                )
            )
            i += 1

    names = [c["decl"]["name"] for c in caps]
    assert len(names) == len(set(names)), "duplicate capability name in corpus"

    doc = {
        "_comment": [
            "GENERATED — do not hand-edit. Source: scripts/gen-planner-caps-large.py",
            "",
            "Adversarial corpus that makes REMOTE_TOP_K actually filter, so that",
            "recall@K measures the retriever instead of being trivially 100%.",
            "",
            f"  {sum(len(v) for v in NEIGHBOURHOODS.values())} near-miss caps crowding the 13 real ones, sharing their",
            "    vocabulary while being the wrong answer.",
            f"  {len(FRENCH)} caps described in French — never an expected answer, so a",
            "    retriever that over-weights them loses recall, and a lexical index",
            "    built on an English catalogue is measurably blind to them.",
            f"  {sum(len(v) for v in FILLER_DOMAINS.values())} unrelated-domain caps. Easy negatives: they exercise the",
            "    bound, not the ranking.",
            "",
            "Loaded by PLANNER_EVAL_CATALOG=large, merged on top of planner_caps.json",
            "so the expected answers stay identical and numbers stay comparable.",
        ],
        "peers": PEERS,
        "caps": caps,
    }

    out = pathlib.Path(__file__).resolve().parent.parent / "crates/node/tests/fixtures/planner_caps_large.json"
    out.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n")
    print(f"{len(caps)} capabilities → {out.relative_to(pathlib.Path.cwd())}")


if __name__ == "__main__":
    main()
