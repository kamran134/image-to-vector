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
cargo run -p i2v-cli -- in.png out.svg                    # ванильный VTracer 1.0
cargo run -p i2v-cli -- in.png out.svg --defringe          # + альфа-осведомлённый фронтенд (v1, нативное разрешение)
cargo run -p i2v-cli -- in.png out.svg --supersample 4      # v2: субпиксельный контур (медленнее, точнее — не для pixel art)
cargo run -p i2v-bench --bin gen_corpus                     # (пере)сгенерировать синтетический корпус
cargo run -p i2v-bench                                      # quality gate: mean_err/p99/SSIM vs vanilla, exit≠0 при регрессии
cargo test --workspace
```

## Статус

- **Модуль A (альфа-канал)** — v1 и v2 реализованы и протестированы:
  `crates/i2v-core/src/lib.rs` (v1, нативное разрешение), `supersample.rs`
  (v2, субпиксельный контур через supersampling — измеримо лучше v1 на всех
  альфа-кейсах, `pixel-art` осознанно исключён).
- **Бенчмарк с метрикой качества** — реализован: рендер SVG обратно в растр
  (`resvg`), ошибка RGBA + SSIM против оригинала, правило приёмки как код,
  подключено в CI. 0 регрессий на 14 файлах корпуса. См. `docs/SPEC.md` §6.
- **Модуль C (регуляризация геометрии)** и **Модуль B (градиенты)** — не
  начаты, см. `docs/SPEC.md`.

## Лицензия

MIT (совместима с VTracer, MIT OR Apache-2.0).
