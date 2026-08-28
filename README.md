# image-to-vector

Расширения к движку [VTracer](https://github.com/visioncortex/vtracer) 1.0,
закрывающие то, чего в нём нет: корректная работа с альфа-каналом
(прозрачные PNG без ореола), геометрическая регуляризация контуров и,
возможно, градиенты.

VTracer 1.0 — это framework с публичными точками расширения (`Frontend`,
`ColorFitter`, `CurvePass`, `CurveFitter`, `OptimizerPass`), поэтому проект
строится как набор плагинов к нему, а не как обёртка и не как форк.

**Статус:** проектирование. Техническое задание — [`docs/SPEC.md`](docs/SPEC.md).

## Лицензия

MIT (совместима с VTracer, MIT OR Apache-2.0).
