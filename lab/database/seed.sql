-- lab/database/seed.sql
-- Test data for the Sistema de Validación de Facturas.
--
-- Usage:
--   psql -h 127.0.0.1 -U lab -d lab -f lab/database/schema.sql
--   psql -h 127.0.0.1 -U lab -d lab -f lab/database/seed.sql
--
-- Idempotent: safe to run multiple times (INSERT ... ON CONFLICT DO NOTHING /
-- ON CONFLICT DO UPDATE). IDs are fixed so the terminal script and docs can
-- reference them.
--
-- Dev partner pre-shared keys (verified against `psk_hash` with Argon2):
--   partner 1 (Farmacia Central):  central-2026
--   partner 2 (Farmacia del Sur):  sur-2026
--   partner 3 (inactive):          inactiva-2026
-- Regenerate hashes with the `hash_psk` test helper in `src/db.rs`.

-- ============================================================
-- Partners (farmacias asociadas)
-- ============================================================

insert into partners (id, name, ruc, psk_hash, is_active) values
    (1, 'Farmacia Central', '80012345-1', '$argon2id$v=19$m=19456,t=2,p=1$lu7T8NyVPRt+vnBa0x9V+Q$UFEkx3P/nz2FvWCCOlQcsS7bPLm/DW+XFCA19b2MyhY', true),
    (2, 'Farmacia del Sur', '80067890-2', '$argon2id$v=19$m=19456,t=2,p=1$uLAQNdp+YZQd5VzlbwaoTg$s5E65818eBZ6q7dNDbbeDjyUubodbyqL8IfgtfmLvaY', true),
    (3, 'Farmacia Inactiva', '80011111-3', '$argon2id$v=19$m=19456,t=2,p=1$Lg/pwyDTZaT3fbtp5WBc/A$RpxtsXZ4GqUeGbb4piMQ6dZ0QtfiBUWMR3K6uVKaGjQ', false)
on conflict (id) do update set
    name = excluded.name,
    ruc = excluded.ruc,
    psk_hash = excluded.psk_hash,
    is_active = excluded.is_active;

-- ============================================================
-- Clients
-- ci is UNIQUE and enough to identify the client; the verification
-- digit (<ci>-<digit> RUC suffix) is optional government-issued data.
-- Every client has at least one OTP channel (phone and/or email).
-- ============================================================

insert into clients (id, ci, verification_digit, first_name, last_name, phone_number, email) values
    (1, 1234567, 1, 'María', 'González', '+595981111111', 'maria@example.com'),
    (2, 2345678, 5, 'Juan', 'Pérez', '+595982222222', null),
    (3, 3456789, null, 'Ana', 'López', null, 'ana@example.com'),
    (4, 4567890, 2, 'Pedro', 'Sosa', '+595983444444', 'pedro@example.com'),
    (5, 5678901, 7, 'Lucía', 'Fernández', '+595984555555', null),
    (6, 6789012, 0, 'Carlos', 'Giménez', null, 'carlos@example.com'),
    (7, 7890123, 4, 'Rosa', 'Ayala', '+595986777777', 'rosa@example.com'),
    (8, 8901234, 9, 'Miguel', 'Torres', '+595987888888', null),
    (9, 9012345, 3, 'Elena', 'Ruiz', null, 'elena@example.com'),
    (10, 1123456, 8, 'Diego', 'Silva', '+595989101010', 'diego@example.com'),
    (11, 2234567, 6, 'Carmen', 'Vega', '+595980111111', null),
    (12, 3344568, 1, 'Hugo', 'Prieto', null, 'hugo@example.com')
on conflict (id) do update set
    ci = excluded.ci,
    verification_digit = excluded.verification_digit,
    first_name = excluded.first_name,
    last_name = excluded.last_name,
    phone_number = excluded.phone_number,
    email = excluded.email;

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
    (6, 'Mochila Lab', 'Premio canjeable, mochila del laboratorio'),
    (7, 'Alcohol 70% 500ml', 'Desinfectante, botella 500ml'),
    (8, 'Jabón Líquido 250ml', 'Higiene, dosificador 250ml'),
    (9, 'Protector Solar FPS50', 'Cuidado personal, tubo 120ml'),
    (10, 'Termo Deportivo 750ml', 'Premio canjeable, termo deportivo 750ml'),
    (11, 'Gorra Lab', 'Premio canjeable, gorra del laboratorio'),
    (12, 'Kit Primeros Auxilios', 'Premio canjeable, kit de primeros auxilios')
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
    (4, 4, 3),
    (5, 7, 6),
    (6, 8, 4),
    (7, 9, 12)
on conflict (id) do update set
    product_id = excluded.product_id,
    points_cost = excluded.points_cost;

-- ============================================================
-- Redeemable products (cost points_needed per unit redeemed)
-- ============================================================

insert into redeemable_products (id, product_id, points_needed) values
    (1, 5, 100),
    (2, 6, 250),
    (3, 3, 50),
    (4, 10, 150),
    (5, 11, 80),
    (6, 12, 300)
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
-- Sample tickets (demo data):
-- partner 1 sells on invoices 001-001-xxxxxxxx, partner 2 on 002-001-xxxxxxxx
-- (invoice numbers are unique per partner).
--
--  id | partner | client | invoice         | lines                          | earned
--   1 |   1     |   1    | 001-001-0000001 | 2x Paracetamol (2x10)            | 25
--   2 |   1     |   2    | 001-001-0000002 | 1x Ibuprofeno (8) + 2x Crema (6) | 14
--   3 |   1     |   3    | 001-001-0000003 | 3x Alcohol (3x6)                 | 18
--   4 |   2     |   1    | 002-001-0000001 | 1x Protector (12)                | 12
--   5 |   1     |   4    | 001-001-0000004 | 2x Paracetamol (2x10)            | 20
--   6 |   2     |   5    | 002-001-0000002 | 5x Jabón (5x4)                   | 20
--   7 |   1     |   6    | 001-001-0000005 | 1x Termo Acero (no promo)        | 0
--   8 |   1     |   7    | 001-001-0000006 | 4x Vitamina C (4x5)              | 20
--   9 |   2     |   8    | 002-001-0000003 | 1x Paracet (10)+1x Ibupr (8)+1x Prot (12) | 30
--  10 |   1     |   9    | 001-001-0000007 | 2x Jabón (2x4)                   | 8
--  11 |   2     |  10    | 002-001-0000004 | 1x Crema (3)                     | 3
--  12 |   1     |  11    | 001-001-0000008 | 6x Alcohol (6x6)                 | 36
--
-- Ticket 7 earns nothing (Termo Acero is redeemable-only), so it posts no
-- ledger row — mirroring what POST /api/v1/partners/tickets does.
-- Only inserted when each ticket id does not exist yet.
-- ============================================================

insert into tickets (id, ticket_id, partner_id, client_id)
select * from (values
    (1, '001-001-0000001', 1, 1),
    (2, '001-001-0000002', 1, 2),
    (3, '001-001-0000003', 1, 3),
    (4, '002-001-0000001', 2, 1),
    (5, '001-001-0000004', 1, 4),
    (6, '002-001-0000002', 2, 5),
    (7, '001-001-0000005', 1, 6),
    (8, '001-001-0000006', 1, 7),
    (9, '002-001-0000003', 2, 8),
    (10, '001-001-0000007', 1, 9),
    (11, '002-001-0000004', 2, 10),
    (12, '001-001-0000008', 1, 11)
) as v(id, ticket_id, partner_id, client_id)
where not exists (select 1 from tickets t where t.id = v.id);

insert into ticket_details (ticket_id, product_id, quantity) values
    (1, 1, 2),
    (1, 3, 1),
    (2, 2, 1),
    (2, 4, 2),
    (3, 7, 3),
    (4, 9, 1),
    (5, 1, 2),
    (6, 8, 5),
    (7, 5, 1),
    (8, 3, 4),
    (9, 1, 1),
    (9, 2, 1),
    (9, 9, 1),
    (10, 8, 2),
    (11, 4, 1),
    (12, 7, 6)
on conflict (ticket_id, product_id) do nothing;

insert into point_ledger (id, client_id, points_delta)
select * from (values
    (1, 1, 25),
    (2, 2, 14),
    (3, 3, 18),
    (4, 1, 12),
    (5, 4, 20),
    (6, 5, 20),
    (7, 7, 20),
    (8, 8, 30),
    (9, 9, 8),
    (10, 10, 3),
    (11, 11, 36)
) as v(id, client_id, points_delta)
where not exists (select 1 from point_ledger pl where pl.id = v.id);

insert into ticket_earnings (id, point_ledger_id, ticket_id)
select * from (values
    (1, 1, 1),
    (2, 2, 2),
    (3, 3, 3),
    (4, 4, 4),
    (5, 5, 5),
    (6, 6, 6),
    (7, 7, 8),
    (8, 8, 9),
    (9, 9, 10),
    (10, 10, 11),
    (11, 11, 12)
) as v(id, point_ledger_id, ticket_id)
where not exists (select 1 from ticket_earnings te where te.id = v.id);

select setval('tickets_id_seq', (select max(id) from tickets));
select setval('ticket_details_id_seq', (select max(id) from ticket_details));
select setval('point_ledger_id_seq', (select max(id) from point_ledger));
select setval('ticket_earnings_id_seq', (select max(id) from ticket_earnings));
