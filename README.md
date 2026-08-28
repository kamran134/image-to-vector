# image-to-vector

Расширения к движку [VTracer](https://github.com/visioncortex/vtracer) 1.0,
закрывающие то, чего в нём нет: корректная работа с альфа-каналом
(прозрачные PNG без ореола), геометрическая регуляризация контуров и,
возможно, градиенты.

VTracer 1.0 — это framework с публичными точками расширения (`Frontend`,
`ColorFitter`, `CurvePass`, `CurveFitter`, `OptimizerPass`), поэтому проект
строится как набор плагинов к нему, а не как обёртка и не как форк.

Rust, консоль — не веб. Техническое задание и статус по каждому пункту —
[`docs/SPEC.md`](docs/SPEC.md).

## Использование

```sh
cargo run -p i2v-cli -- in.png out.svg              # ванильный VTracer 1.0
cargo run -p i2v-cli -- in.png out.svg --defringe    # + альфа-осведомлённый фронтенд (i2v-core)
cargo run -p i2v-bench                               # таблица paths/colors/bytes по crates/i2v-bench/corpus/
cargo test --workspace
```

## Статус

- **Модуль A (альфа-канал)** — реализован (v1, нативное разрешение) и
  протестирован: `crates/i2v-core/src/lib.rs`.
- **Модуль C (регуляризация геометрии)** и **Модуль B (градиенты)** — не
  начаты, см. `docs/SPEC.md`.

## Лицензия

MIT (совместима с VTracer, MIT OR Apache-2.0).
