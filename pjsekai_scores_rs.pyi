from typing import Any, Mapping, NoReturn, Optional, Protocol, Sequence, TypedDict, Union


class _ReadableText(Protocol):
    def read(self) -> str: ...


class _ReadlinesText(Protocol):
    def readlines(self) -> Sequence[str]: ...


_TextSource = Union[str, _ReadableText, _ReadlinesText]
_JsonSource = Union[str, Mapping[str, Any], _ReadableText, _ReadlinesText]
_FractionLike = Union["Fraction", int, float, str]


class MusicMetaDict(TypedDict, total=False):
    fever_end_time: float
    fever_score: float
    skill_score_solo: Sequence[float]
    skill_score_multi: Sequence[float]


class Fraction:
    def __init__(self, numerator: int, denominator: int = ...) -> None: ...

    @property
    def numerator(self) -> int: ...

    @property
    def denominator(self) -> int: ...

    def limit_denominator(self, max_denominator: Optional[int] = ...) -> "Fraction": ...
    def __float__(self) -> float: ...
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...


class Meta:
    def __new__(cls) -> NoReturn: ...

    @property
    def title(self) -> Optional[str]: ...

    @property
    def subtitle(self) -> Optional[str]: ...

    @property
    def artist(self) -> Optional[str]: ...

    @property
    def genre(self) -> Optional[str]: ...

    @property
    def designer(self) -> Optional[str]: ...

    @property
    def difficulty(self) -> Optional[str]: ...

    @property
    def playlevel(self) -> Optional[str]: ...

    @property
    def songid(self) -> Optional[str]: ...

    @property
    def wave(self) -> Optional[str]: ...

    @property
    def waveoffset(self) -> Optional[str]: ...

    @property
    def jacket(self) -> Optional[str]: ...

    @property
    def background(self) -> Optional[str]: ...

    @property
    def movie(self) -> Optional[str]: ...

    @property
    def movieoffset(self) -> Optional[float]: ...

    @property
    def basebpm(self) -> Optional[float]: ...


class Event:
    def __new__(cls) -> NoReturn: ...

    @property
    def bar(self) -> Fraction: ...

    @property
    def bpm(self) -> Optional[Fraction]: ...

    @property
    def bar_length(self) -> Optional[Fraction]: ...

    @property
    def sentence_length(self) -> Optional[int]: ...

    @property
    def speed(self) -> Optional[float]: ...

    @property
    def section(self) -> Optional[str]: ...

    @property
    def text(self) -> Optional[str]: ...


class Score:
    def __new__(cls) -> NoReturn: ...

    @staticmethod
    def open(path: str) -> "Score": ...

    @staticmethod
    def open_sus(path: str) -> "Score": ...

    @staticmethod
    def open_json(path: str) -> "Score": ...

    @staticmethod
    def from_str(content: str) -> "Score": ...

    @staticmethod
    def from_json(value: _JsonSource) -> "Score": ...

    @staticmethod
    def from_dict(dict: Mapping[str, Any]) -> "Score": ...

    @staticmethod
    def load(value: _JsonSource) -> "Score": ...

    @property
    def meta(self) -> Meta: ...

    def set_meta(
        self,
        title: Optional[str] = ...,
        artist: Optional[str] = ...,
        difficulty: Optional[str] = ...,
        playlevel: Optional[str] = ...,
        jacket: Optional[str] = ...,
        songid: Optional[str] = ...,
        subtitle: Optional[str] = ...,
    ) -> None: ...

    def note_count(self) -> int: ...
    def event_count(self) -> int: ...
    def title(self) -> Optional[str]: ...
    def artist(self) -> Optional[str]: ...
    def difficulty(self) -> Optional[str]: ...
    def playlevel(self) -> Optional[str]: ...
    def events(self) -> list[Event]: ...
    def get_time(self, bar: _FractionLike) -> Fraction: ...
    def get_event(self, bar: _FractionLike) -> Event: ...
    def get_time_delta(self, bar_from: _FractionLike, bar_to: _FractionLike) -> Fraction: ...
    def get_bar_by_time(self, time: float) -> Fraction: ...


class Lyric:
    def __new__(cls) -> NoReturn: ...

    @staticmethod
    def load(content: _TextSource) -> "Lyric": ...

    def word_count(self) -> int: ...


class Rebase:
    def __new__(cls) -> NoReturn: ...

    @staticmethod
    def load(value: _JsonSource) -> "Rebase": ...

    @staticmethod
    def from_json(json_str: str) -> "Rebase": ...

    @staticmethod
    def from_dict(dict: Mapping[str, Any]) -> "Rebase": ...

    @staticmethod
    def load_from_dict(dict: Mapping[str, Any]) -> "Rebase": ...

    def apply(self, score: Score) -> Score: ...
    def rebase(self, score: Score) -> Score: ...
    def __call__(self, score: Score) -> Score: ...


class Drawing:
    def __init__(
        self,
        score: Optional[Score] = ...,
        lyric: Optional[Lyric] = ...,
        style_sheet: Optional[str] = ...,
        note_host: Optional[str] = ...,
        skill: bool = ...,
        music_meta: Optional[MusicMetaDict] = ...,
        target_segment_seconds: Optional[float] = ...,
        generator: Optional[str] = ...,
        note_asset_extension: Optional[str] = ...,
        font_paths: Optional[Sequence[str]] = ...,
        font_dirs: Optional[Sequence[str]] = ...,
    ) -> None: ...

    def svg(self, score: Optional[Score] = ..., lyric: Optional[Lyric] = ...) -> str: ...
    def png(self, score: Optional[Score] = ..., lyric: Optional[Lyric] = ...) -> bytes: ...
    def jpg(
        self,
        score: Optional[Score] = ...,
        lyric: Optional[Lyric] = ...,
        jpeg_quality: int = ...,
    ) -> bytes: ...
    def jpeg(
        self,
        score: Optional[Score] = ...,
        lyric: Optional[Lyric] = ...,
        jpeg_quality: int = ...,
    ) -> bytes: ...

    @property
    def note_size(self) -> int: ...

    @note_size.setter
    def note_size(self, v: int) -> None: ...

    @property
    def time_height(self) -> float: ...

    @time_height.setter
    def time_height(self, v: float) -> None: ...

    @property
    def lane_width(self) -> int: ...

    @lane_width.setter
    def lane_width(self, v: int) -> None: ...

    def set_font_paths(self, paths: Sequence[str]) -> None: ...
    def add_font_path(self, path: str) -> None: ...
    def set_font_dirs(self, dirs: Sequence[str]) -> None: ...
    def add_font_dir(self, dir: str) -> None: ...


def sus_to_svg(
    sus_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
) -> str: ...


def sus_to_png(
    sus_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
) -> bytes: ...


def sus_to_jpg(
    sus_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
    jpeg_quality: int = ...,
) -> bytes: ...


def sus_to_jpeg(
    sus_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
    jpeg_quality: int = ...,
) -> bytes: ...


def score_to_svg(
    score_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
) -> str: ...


def score_to_png(
    score_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
) -> bytes: ...


def score_to_jpg(
    score_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
    jpeg_quality: int = ...,
) -> bytes: ...


def score_to_jpeg(
    score_path: str,
    note_host: Optional[str] = ...,
    style_sheet: Optional[str] = ...,
    rebase_json: Optional[str] = ...,
    lyric_content: Optional[str] = ...,
    skill: bool = ...,
    music_meta: Optional[MusicMetaDict] = ...,
    target_segment_seconds: Optional[float] = ...,
    generator: Optional[str] = ...,
    note_asset_extension: Optional[str] = ...,
    font_paths: Optional[Sequence[str]] = ...,
    font_dirs: Optional[Sequence[str]] = ...,
    jpeg_quality: int = ...,
) -> bytes: ...
