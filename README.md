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

## Problema a resolver

Un laboratorio de medicamentos (lab) cuenta con un programa de fidelización de
clientes que consiste en el canje de productos adquiribles en las farmacias
asociadas (farm) por puntos acumulables. Luego estos mismos puntos pueden
cambiarse por otros productos habilitados.

Hasta ahora, el procesamiento de estos canjes se realizaba de forma manual. Una
persona debía verificar la existencia de la compra realizada, revisar una tabla
de canje de puntos por productos, actualizar los puntos acumulados del cliente.
Verificar que los intercambios no puedan realizarse más de una sóla vez por
venta, y recepcionar los cambios de productos por puntos y actualizar el saldo
del cliente.

El sistema debe automatizar este proceso y de ser posible, simplificarlo.

## Especificaciones

- El sistema **DEBE** permitir la carga y anulación de facturas por parte de una
  farmacia asociada para el intercambio de automático de puntos en los productos
  válidos.
- El sistema **DEBE** permitir a los clientes del laboratorio el intercambio de
  sus puntos por productos intercambiables.
- El sistema **DEBE** permitir a los clientes autenticarse para realizar el
  canje.
- El sistema **DEBE** permitir a las farmacias asociadas al laboratorio
  autenticar sus peticiones.

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
