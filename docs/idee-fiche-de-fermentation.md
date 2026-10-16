# Idée : fiche de fermentation en ligne

Notée le 2026-10-07. Pas commencée, à reprendre plus tard. Spec à écrire
dans `docs/superpowers/specs/` avant de coder.

## Le principe

1. Un chercheur remplit une **fiche de fermentation** dans un formulaire web :
   souche, milieu, lot, objectif, paramètres (µ, Y_x/s, m_s, X₀, V₀, S_f),
   profil d'alimentation, date souhaitée, responsable.
2. La fiche est enregistrée dans une **base de données**.
3. Une **notification part sur le canal Teams** des techniciens.
4. Fermentool récupère la fiche et **prépare le run** : l'écran New run est
   prérempli, le technicien vérifie et lance lui-même. Un run n'est jamais
   démarré à distance.
5. Pendant le run, la fiche s'enrichit : **prélèvements** (OD600, DCW,
   glucose résiduel, volume prélevé), **annotations** (induction, mousse,
   antimousse).
6. À la fin, un **bilan** : masse délivrée par la balance, µ réel comparé au
   µ de consigne, rendement apparent, export (CSV, PDF) pour le cahier de
   labo.

Pourquoi : Fermentool devient le registre des expériences du labo, pas
seulement le pilote de la pompe. Les prélèvements ouvrent aussi la porte au
recalage du µ en cours de run (voir plus bas).

## Hébergement : le NAS Synology du labo

Retenu plutôt que le sous-domaine `delvigne-lab.com` chez o2switch :

- les données restent au labo (souches, protocoles, projets sous contrat) ;
- le PC des runs est sur le même réseau, il interroge le NAS directement,
  sans relais sur Internet ;
- de l'extérieur, accès par le VPN de l'université, rien d'exposé en public ;
- le NAS envoie la notification Teams en HTTPS sortant.

Précautions :

- des comptes et le pare-feu DSM **même derrière le VPN** : le réseau
  interne, c'est tout le campus ;
- une sauvegarde automatique de la base (Hyper Backup) : les fiches
  deviennent l'historique du labo.

À vérifier :

- le **modèle du NAS** : Container Manager (Docker, modèles « + » en
  général) ou, sinon, Web Station (PHP) + MariaDB ;
- que le PC des runs atteint le NAS (sous-réseaux, pare-feu de
  l'université) ;
- que le NAS peut sortir en HTTPS vers Teams.

## Notification Teams

Webhook Workflows « publier dans un canal quand une requête webhook est
reçue » (modèle tout fait, contrairement aux messages privés), avec une
carte Adaptive Card comme celles de `notify.rs`, et un bouton qui ouvre la
fiche. Envoyée par le serveur qui stocke la fiche, pas par le daemon : la
notification part même si le PC des runs est éteint.

## Côté Fermentool

- Le daemon va chercher les fiches sur le NAS, sur son propre thread, jamais
  sur le thread de contrôle (même règle que `notify.rs`).
- Un recalage de la courbe à partir des prélèvements doit rester compatible
  avec la reprise après plantage : `value_at` reste pure, un recalage devient
  un nouveau segment journalisé, ancré à un temps écoulé.

## Questions ouvertes

- L'accès à distance depuis le téléphone (point 11 des notes de session,
  prévu sur o2switch) pourrait aussi passer par le NAS. Plus cohérent, mais
  le téléphone devra être sur le VPN pour acquitter une alarme ou arrêter
  un run.
- Le contenu exact de la fiche : à définir avec les chercheurs et les
  techniciens.
