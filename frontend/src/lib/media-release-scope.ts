export type LocalReleaseScopeKind =
  | "season_episode"
  | "season_only"
  | "season_pack"
  | "episode_only"
  | "absolute_episode"
  | "unmatched";

type SeasonEpisodeRange = {
  season: number;
  start: number;
  end: number;
};

export type LocalReleaseScope = {
  kind: LocalReleaseScopeKind;
  label: string;
  seasons: number[];
  episodeRanges: SeasonEpisodeRange[];
  episodes: number[];
  absoluteEpisodes: number[];
  searchAliases: string[];
};

const STANDARD_SEASON_EPISODE =
  /(?:^|[^a-z0-9])s(?:eason)?[\s._\[\]()-]*0*(\d{1,3})[\s._\[\]()-]*e(?:p(?:isode)?)?[\s._\[\]()-]*0*(\d{1,4})(?!\d)(?:(?:[\s._]*(?:-|~|to)[\s._]*(?:e(?:p(?:isode)?)?[\s._\[\]()-]*)?0*(\d{1,4})(?!\d))|((?:[\s._\[\]()-]*e(?:p(?:isode)?)?[\s._\[\]()-]*0*\d{1,4}(?!\d))+))?/gi;
const CHINESE_SEASON_EPISODE =
  /第\s*0*(\d{1,3})\s*季\s*(?:第\s*)?0*(\d{1,4})(?:\s*(?:-|~|至|到)\s*0*(\d{1,4}))?\s*[集话]/g;
const STANDARD_SEASON = /(?:^|[^a-z0-9])s(?:eason)?[\s._-]*0*(\d{1,3})(?!\d)/gi;
const CHINESE_SEASON = /第\s*0*(\d{1,3})\s*季/g;
const STANDARD_EPISODE =
  /(?:^|[^a-z0-9])e(?:p(?:isode)?)?[\s._-]*0*(\d{1,4})(?!\d)(?:[\s._]*(?:-|~|to)[\s._]*(?:e(?:p(?:isode)?)?[\s._-]*)?0*(\d{1,4})(?!\d))?/gi;
const CHINESE_EPISODE = /第\s*0*(\d{1,4})(?:\s*(?:-|~|至|到)\s*0*(\d{1,4}))?\s*[集话]/g;
const ABSOLUTE_EPISODE = /(?:^|[^a-z0-9])abs(?:olute)?[\s._-]*0*(\d{1,4})(?!\d)/gi;
const ANIME_DASH_EPISODE = /(?:^|[\s._])-\s*0*(\d{1,4})(?:v\d+)?(?=\s*(?:[[(._]|$))/gi;
const SEASON_PACK = /\b(?:complete|batch|season[\s._-]*pack)\b|全集|全季|整季|季度合集/i;

function parsedNumber(value: string | undefined): number | null {
  if (value == null) return null;
  const parsed = Number.parseInt(value, 10);
  return Number.isInteger(parsed) && parsed >= 0 ? parsed : null;
}

function sortedUnique(values: number[]): number[] {
  return [...new Set(values)].sort((left, right) => left - right);
}

function addRange(
  ranges: SeasonEpisodeRange[],
  seasonValue: string | undefined,
  startValue: string | undefined,
  endValue?: string,
) {
  const season = parsedNumber(seasonValue);
  const start = parsedNumber(startValue);
  const parsedEnd = parsedNumber(endValue);
  if (season == null || start == null) return;
  const end = parsedEnd ?? start;
  ranges.push({ season, start: Math.min(start, end), end: Math.max(start, end) });
}

function readNumbers(value: string, pattern: RegExp, group = 1): number[] {
  const numbers: number[] = [];
  pattern.lastIndex = 0;
  for (const match of value.matchAll(pattern)) {
    const parsed = parsedNumber(match[group]);
    if (parsed != null) numbers.push(parsed);
  }
  return sortedUnique(numbers);
}

function padded(prefix: string, value: number): string {
  return `${prefix}${String(value).padStart(2, "0")}`;
}

function rangeLabel(range: SeasonEpisodeRange): string {
  const start = `${padded("S", range.season)}${padded("E", range.start)}`;
  return range.start === range.end ? start : `${start}-${padded("E", range.end)}`;
}

function limitedLabels(values: string[]): string {
  if (values.length <= 3) return values.join("、");
  return `${values.slice(0, 3).join("、")} +${values.length - 3}`;
}

function rangeValues(start: number, end: number): number[] {
  if (end - start > 500) return [start, end];
  return Array.from({ length: end - start + 1 }, (_, index) => start + index);
}

function aliasesForScope(
  seasons: number[],
  ranges: SeasonEpisodeRange[],
  episodes: number[],
  absoluteEpisodes: number[],
  label: string,
): string[] {
  const aliases = new Set<string>([label]);
  for (const season of seasons) {
    aliases.add(padded("S", season));
    aliases.add(`S${season}`);
    aliases.add(`Season ${season}`);
    aliases.add(`第${season}季`);
  }
  for (const range of ranges) {
    for (const episode of rangeValues(range.start, range.end)) {
      aliases.add(`${padded("S", range.season)}${padded("E", episode)}`);
      aliases.add(`S${range.season}E${episode}`);
      aliases.add(`S${range.season}${padded("E", episode)}`);
      aliases.add(`${padded("S", range.season)}E${episode}`);
      aliases.add(`Season ${range.season} Episode ${episode}`);
      aliases.add(`第${range.season}季第${episode}集`);
    }
  }
  for (const episode of episodes) {
    aliases.add(padded("E", episode));
    aliases.add(`E${episode}`);
    aliases.add(`EP${episode}`);
    aliases.add(`第${episode}集`);
    aliases.add(`第${episode}话`);
  }
  for (const episode of absoluteEpisodes) {
    aliases.add(`ABS${episode}`);
    aliases.add(`Absolute ${episode}`);
    aliases.add(`绝对集 ${episode}`);
  }
  return [...aliases];
}

export function parseLocalReleaseScope(title: string): LocalReleaseScope {
  const episodeRanges: SeasonEpisodeRange[] = [];
  STANDARD_SEASON_EPISODE.lastIndex = 0;
  for (const match of title.matchAll(STANDARD_SEASON_EPISODE)) {
    addRange(episodeRanges, match[1], match[2], match[3]);
    if (match[4]) {
      const adjacentEpisodes = readNumbers(match[4], /e(?:p(?:isode)?)?[\s._\[\]()-]*0*(\d{1,4})(?!\d)/gi);
      for (const episode of adjacentEpisodes) addRange(episodeRanges, match[1], String(episode));
    }
  }

  CHINESE_SEASON_EPISODE.lastIndex = 0;
  for (const match of title.matchAll(CHINESE_SEASON_EPISODE)) {
    addRange(episodeRanges, match[1], match[2], match[3]);
  }

  const seasons = sortedUnique([
    ...readNumbers(title, STANDARD_SEASON),
    ...readNumbers(title, CHINESE_SEASON),
    ...episodeRanges.map((range) => range.season),
  ]);

  if (episodeRanges.length > 0) {
    const labels = episodeRanges.map(rangeLabel);
    const label = limitedLabels([...new Set(labels)]);
    return {
      kind: "season_episode",
      label,
      seasons,
      episodeRanges,
      episodes: [],
      absoluteEpisodes: [],
      searchAliases: aliasesForScope(seasons, episodeRanges, [], [], label),
    };
  }

  if (seasons.length > 0) {
    const seasonLabels = seasons.map((season) => padded("S", season));
    const isPack = SEASON_PACK.test(title);
    const label = `${limitedLabels(seasonLabels)} · ${isPack ? "全季" : "无集数"}`;
    return {
      kind: isPack ? "season_pack" : "season_only",
      label,
      seasons,
      episodeRanges: [],
      episodes: [],
      absoluteEpisodes: [],
      searchAliases: aliasesForScope(seasons, [], [], [], label),
    };
  }

  const episodes = sortedUnique([
    ...readNumbers(title, STANDARD_EPISODE),
    ...readNumbers(title, STANDARD_EPISODE, 2),
    ...readNumbers(title, CHINESE_EPISODE),
    ...readNumbers(title, CHINESE_EPISODE, 2),
  ]);
  if (episodes.length > 0) {
    const label = `${limitedLabels(episodes.map((episode) => padded("E", episode)))} · 无季数`;
    return {
      kind: "episode_only",
      label,
      seasons: [],
      episodeRanges: [],
      episodes,
      absoluteEpisodes: [],
      searchAliases: aliasesForScope([], [], episodes, [], label),
    };
  }

  const absoluteEpisodes = sortedUnique([
    ...readNumbers(title, ABSOLUTE_EPISODE),
    ...readNumbers(title, ANIME_DASH_EPISODE),
  ]);
  if (absoluteEpisodes.length > 0) {
    const label = `绝对集 ${limitedLabels(absoluteEpisodes.map(String))}`;
    return {
      kind: "absolute_episode",
      label,
      seasons: [],
      episodeRanges: [],
      episodes: [],
      absoluteEpisodes,
      searchAliases: aliasesForScope([], [], [], absoluteEpisodes, label),
    };
  }

  return {
    kind: "unmatched",
    label: "未识别季集",
    seasons: [],
    episodeRanges: [],
    episodes: [],
    absoluteEpisodes: [],
    searchAliases: ["未识别季集"],
  };
}

export function matchesLocalResourceQuery(fields: string[], scope: LocalReleaseScope, query: string): boolean {
  const terms = query.normalize("NFKC").trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const searchable = [...fields, ...scope.searchAliases].join(" ").normalize("NFKC").toLocaleLowerCase();
  const compactSearchable = searchable.replace(/[\s._\[\]()-]+/g, "");
  return terms.every((term) => {
    const compactTerm = term.replace(/[\s._\[\]()-]+/g, "");
    return searchable.includes(term) || (compactTerm.length > 0 && compactSearchable.includes(compactTerm));
  });
}
