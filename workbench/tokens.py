"""One tokenizer for the private (社内資料) search: Latin words of 2+ characters and Japanese bigrams.
Used both for the in-memory match (core.terms) and for the FTS5 index text (store), so both agree on what a hit is."""
import re

_LATIN = re.compile(r'[a-z0-9]{2,}')
_JAPANESE = re.compile(r'[一-龯ぁ-んァ-ヶー]{2,}')


def runs(text):
    """Ordered token runs: each Latin word is its own run; each Japanese run becomes its bigram sequence."""
    text = text.lower()
    out = []
    for match in re.finditer(r'[a-z0-9]{2,}|[一-龯ぁ-んァ-ヶー]{2,}', text):
        word = match.group(0)
        if _LATIN.fullmatch(word):
            out.append([word])
        else:
            out.append([word[i:i + 2] for i in range(len(word) - 1)])
    return out


def terms(text):
    """Set of match units (kept identical to the historical core.terms)."""
    lower = text.lower()
    latin = _LATIN.findall(lower)
    japanese = _JAPANESE.findall(text)
    return set(latin + [s[i:i + 2] for s in japanese for i in range(len(s) - 1)])


def fts_text(text):
    """Space-separated tokens for the FTS5 (unicode61) index: bigrams keep their order so phrase queries find substrings."""
    return ' '.join(token for run in runs(text) for token in run)


def fts_query_all(text):
    """FTS5 MATCH expression for the first pass: the document contains every run of the query (each run a phrase = substring)."""
    units = []
    for run in runs(text):
        unit = '"' + ' '.join(run) + '"'
        if unit not in units:
            units.append(unit)
    return ' AND '.join(units)


def fts_query(text):
    """FTS5 MATCH expression for the second pass: every run as a phrase (adjacent bigrams = substring) OR every single token (recall as before)."""
    phrases, singles = [], []
    for run in runs(text):
        if len(run) > 1:
            phrases.append('"' + ' '.join(run) + '"')
        for token in run:
            if token not in singles:
                singles.append(token)
    parts = phrases + ['"' + token + '"' for token in singles]
    return ' OR '.join(parts)
