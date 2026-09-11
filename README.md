# Sistema de validación de facturas

Este es un sistema prototipo. No apto para uso comercial. Hecho con fines
educativos.

## Objetivos

- Aplicar las técnicas de análisis de sistemas aprendidas en clase
- Demostrar el criterio adquirido en la aplicación de técnicas de análisis de
  sistemas.
- Aprender a crear infraestructuas backend con Rust, Axum, Postgres y HTMX.

## Stack

- Rust
- Axum
- Postgres
- HTMX
- Tailwind CSS
- SQLx

## Funcionamiento

El prototipo consiste en dos servidores web que exponen una REST API. Uno
corresponde al servicio de validación de facturas (farm) y otro que solicita la
validación y al que los usuarios terminan subiendo sus facturas (lab).

Un cliente puede ingresar al sistema a través de los endpoints especializados
para tal efecto. Luego el servidor sirve el HTML y el CSS compilados y el
cliente puede subir sus facturas para canjearlas por puntos, o cambiar sus
puntos por productos seleccionados.

Para validar las facturas, `lab` consulta a `farm` si efectivamente esta compra
se dio. `farm` responde con los datos de la factura si esta existe. Sino,
retorna un HTTP 404.

## Integrantes

- Santiago Wu <santiago.wu.chamorro@gmail.com>

## Política de Uso de IA

Si bien el uso de IA permite acelerar el desarrollo de los proyectos, en este
repositorio **NO SE ACEPTARÁN CONTRIBUCIONES REALIZADAS TOTAL O PARCIALMENTE CON
IA**. Este es un proyecto con _fines didácticos y de aprendizaje_, por lo que
creo conveniente y deseable **hacer un esfuerzo** por aprender el funcionamiento
del mismo y sus tecnologías. Nadie inició siendo un experto en todo y yo no soy
la excepción. Solamente empecé antes a estudiar, y eso me da una pequeña
ventaja, pero también pueden alcanzarme si se esfuerzan.

## Licenciamiento

Este proyecto se distribuye bajo la licencia [GPL3](./LICENSE)
