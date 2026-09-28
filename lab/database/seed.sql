-- lab/database/seed.sql
-- Test data for the Sistema de Validación de Facturas.
--
-- Usage:
--   psql -h 127.0.0.1 -U lab -d lab -f lab/database/seed.sql
--
-- Idempotent: safe to run multiple times (INSERT ... ON CONFLICT DO NOTHING).
-- IDs are fixed so the terminal script and docs can reference them.

-- ============================================================
-- Partners (farmacias asociadas)
-- ============================================================

insert into partners (id, name, ruc, is_active) values
    (1, 'Farmacia Central', '80012345-1', true),
    (2, 'Farmacia del Sur', '80067890-2', true),
    (3, 'Farmacia Inactiva', '80011111-3', false)
on conflict (id) do update set
    name = excluded.name,
    ruc = excluded.ruc,
    is_active = excluded.is_active;

-- ============================================================
-- Clients
-- ci is UNIQUE and enough to identify the client; the verification
-- digit (<ci>-<digit> RUC suffix) is optional government-issued data.
-- ============================================================

insert into clients (id, ci, verification_digit, first_name, last_name) values
    (1, 1234567, 1, 'María', 'González'),
    (2, 2345678, 5, 'Juan', 'Pérez'),
    (3, 3456789, null, 'Ana', 'López')
on conflict (id) do update set
    ci = excluded.ci,
    verification_digit = excluded.verification_digit,
    first_name = excluded.first_name,
    last_name = excluded.last_name;

-- ============================================================
-- Products
-- 1-4 earn points (promotion), 3/5/6 are redeemable, 4 is earn-only.
-- ============================================================

insert into products (id, name, description) values
    (1, 'Paracetamol 500mg', 'Analgésico, caja x 16 comprimidos'),
    (2, 'Ibuprofeno 400mg', 'Antiinflamatorio, caja x 10 cápsulas'),
    (3, 'Vitamina C 1g', 'Suplemento, tubo x 10 efervescentes'),
    (4, 'Crema Hidratante', 'Cuidado personal, frasco 200ml'),
    (5, 'Termo Acero 1L', 'Premio canjeable, termo de acero 1L'),
    (6, 'Mochila Lab', 'Premio canjeable, mochila del laboratorio')
on conflict (id) do update set
    name = excluded.name,
    description = excluded.description;

-- ============================================================
-- Promotion products (earn points_cost per unit purchased)
-- ============================================================

insert into promotion_products (id, product_id, points_cost) values
    (1, 1, 10),
    (2, 2, 8),
    (3, 3, 5),
    (4, 4, 3)
on conflict (id) do update set
    product_id = excluded.product_id,
    points_cost = excluded.points_cost;

-- ============================================================
-- Redeemable products (cost points_needed per unit redeemed)
-- ============================================================

insert into redeemable_products (id, product_id, points_needed) values
    (1, 5, 100),
    (2, 6, 250),
    (3, 3, 50)
on conflict (id) do update set
    product_id = excluded.product_id,
    points_needed = excluded.points_needed;

-- Keep serial sequences in sync with the fixed IDs above.
select setval('partners_id_seq', (select max(id) from partners));
select setval('clients_id_seq', (select max(id) from clients));
select setval('products_id_seq', (select max(id) from products));
select setval('promotion_products_id_seq', (select max(id) from promotion_products));
select setval('redeemable_products_id_seq', (select max(id) from redeemable_products));

-- ============================================================
-- Sample ticket (optional demo data):
-- partner 1 sells client 1: 2x Paracetamol (2x10) + 1x Vitamina C (1x5)
-- earned_points = 25.
-- Only inserted when ticket id 1 does not exist yet.
-- ============================================================

insert into tickets (id, partner_id, client_id)
select 1, 1, 1
where not exists (select 1 from tickets where id = 1);

insert into ticket_details (ticket_id, product_id, quantity) values
    (1, 1, 2),
    (1, 3, 1)
on conflict (ticket_id, product_id) do nothing;

insert into point_earnings (ticket_id, earned_points)
select 1, 25
where not exists (select 1 from point_earnings where ticket_id = 1);

select setval('tickets_id_seq', (select max(id) from tickets));
select setval('ticket_details_id_seq', (select max(id) from ticket_details));
select setval('point_earnings_id_seq', (select max(id) from point_earnings));
