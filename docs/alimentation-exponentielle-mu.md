# Alimentation exponentielle, mode « rate µ »

Fiche de référence pour construire une courbe d'alimentation fed-batch dans
Fermentool (*New run* → Curve **Exponential** → mode **rate µ**).

---

## 1. Ce que fait le mode

La consigne suit une exponentielle pure :

```
consigne(t) = Start · e^(µ · t)          (t en heures)
```

- `Start` = valeur au démarrage (rpm ou ml/min selon *Control*)
- `µ` = taux de croissance spécifique imposé (h⁻¹)
- Fermentool ne fait **qu'exécuter** cette courbe. Le choix de `Start` et `µ`
  est un travail de conception en amont (§4).

Tant que `µ` est constant, `F = f(t)` est une exponentielle. Un feed **linéaire**
ne maintient PAS un µ constant (µ décroît au cours du temps).

---

## 2. Comprendre µ

À chaque heure la valeur est **multipliée par `e^µ`**. Croissance relative :
plus la valeur est grande, plus elle monte vite.

Repère le plus parlant : **doublement tous les `ln(2)/µ ≈ 0,693/µ` heures**.

| µ (h⁻¹) | × par heure | temps de doublement |
|--------:|:-----------:|:-------------------:|
| 0,05    | ×1,05       | ~14 h               |
| 0,10    | ×1,11       | ~7 h                |
| 0,15    | ×1,16       | ~4,6 h              |
| 0,20    | ×1,22       | ~3,5 h              |
| 0,30    | ×1,35       | ~2,3 h              |
| 0,69    | ×2,0        | 1 h                 |

> Choisir `µ` = une **fraction de µmax** de la souche dans tes conditions
> (E. coli : µmax ≈ 0,5–0,7 h⁻¹ → on vise souvent 0,1–0,3).

---

## 3. Les deux modes de la courbe Exponential

| Mode | Tu saisis | Fermentool calcule |
|------|-----------|--------------------|
| **start → end** | `Start`, `End`, durée | le µ implicite : `µ = ln(End/Start) / durée_h` |
| **rate µ**      | `Start`, `µ`, durée   | le End : `End = Start · e^(µ · durée_h)` |

En mode **rate µ** tu pilotes l'agressivité de la montée ; le point d'arrivée
en **découle** (et peut être énorme, voir §6).

---

## 4. Calculer `Start` depuis ta souche (calculateur F₀)

Dans Fermentool : panneau dépliable **« Fed-batch F₀ from strain parameters »**
(visible en mode *rate µ*).

### 4.1 Données à réunir

| Symbole | Signification | Comment l'obtenir |
|---------|---------------|-------------------|
| **µ**       | taux de croissance **imposé** (h⁻¹) | tu le choisis (fraction de µmax) |
| **X₀**      | biomasse au **début du feed** (g/L) | DO₆₀₀ × ton facteur DO→g/L, ou poids sec |
| **V₀**      | volume de culture au début du feed (L) | lu sur le réacteur |
| **Y_{x/s}** | rendement biomasse/substrat (g/g) | littérature souche+substrat, ou ΔX/ΔS mesuré en batch. Glucose/E. coli ≈ 0,45 |
| **S_f**     | substrat limitant **dans le flacon de feed** (g/L) | tu l'as préparé → connu (ex. 500 g/L glucose) |
| **m_s** *(option)* | coefficient de maintenance (g substrat · g biomasse⁻¹ · h⁻¹) | littérature souche. E. coli/glucose ≈ 0,02–0,04. Vide → forme croissance seule |
| **V_max** *(option)* | volume max du réacteur (L) | fiche réacteur, sert au `t_max` |

### 4.2 Formule

```
F₀ [L/h]     = (µ / Y_{x/s} + m_s) · X₀ · V₀ / S_f
F₀ [ml/min]  = F₀[L/h] × 1000 / 60
```

`m_s` vide → terme `µ / Y_{x/s}` seul (forme croissance seule). Le renseigner
ajoute typiquement 5–10 % de débit (E. coli à µ modéré).

Hypothèses : `µ` et `Y_{x/s}` constants, culture **substrat-limitante**
(S ≈ 0, tout le substrat entrant est consommé). Volume perdu par évaporation /
prélèvements et ajouté par la régulation de pH non pris en compte dans `t_max`.

→ **`Start` = F₀ en ml/min** (mode *Control* = **ml/min**), `µ` = ton µ.

### 4.3 Durée limitée par le réacteur

Le feed ajoute du volume. En alimentation exponentielle :

```
V(t)  = V₀ + (F₀/µ) · (e^(µ·t) − 1)
t_max = (1/µ) · ln( 1 + µ · (V_max − V₀) / F₀ )      [heures]
```

`t_max` = moment où la phase exponentielle remplit le réacteur → borne haute
pour *Duration*. (En pratique le **kLa / transfert d'O₂** limite souvent avant.)

---

## 5. Exemple chiffré

| Paramètre | Valeur |
|-----------|--------|
| µ         | 0,15 h⁻¹ |
| X₀        | 3 g/L |
| V₀        | 1,5 L |
| Y_{x/s}   | 0,45 g/g |
| S_f       | 500 g/L |
| V_max     | 3 L |

**F₀** (`m_s` laissé vide, forme croissance seule)

```
F₀ = (0,15 / 0,45) × 3 × 1,5 / 500
   = 0,3333 × 4,5 / 500
   = 0,003 L/h
   = 0,05 ml/min
```

Avec `m_s = 0,025` : F₀ = (0,3333 + 0,025) × 4,5 / 500 ≈ 0,00323 L/h ≈ 0,054 ml/min.

**t_max**

```
t_max = (1/0,15) · ln( 1 + 0,15 × (3 − 1,5) / 0,003 )
      = 6,67 · ln(1 + 75)
      = 6,67 × 4,33
      ≈ 29 h
```

**À saisir dans Fermentool** : Control = `ml/min` · Start = `0,05` · µ = `0,15`
· Duration = `29`. (Le bouton **« Use → »** du panneau le fait automatiquement.)

Évolution : `0,05 · e^(0,15·t)` ml/min → à 20 h ≈ `1,0 ml/min`, à 29 h ≈ `3,9 ml/min`.

---

## 6. Pièges

- **L'exponentielle explose.** `Start 5 · µ 0,15 · 24 h` → End ≈ `5·e³·⁶ ≈ 183`.
  Sur 100 h → des millions. La consigne est **bornée** à la plage pompe
  (0,1–350 rpm / 0–99999 ml/min) : au-delà, la courbe fait un **palier plat**
  à la limite. Elle atteint le plafond `L` à `t = ln(L/Start) / µ`.
  → Regarde toujours l'**aperçu** (il affiche le End réel) avant de lancer.
- **Linéaire ≠ µ constant.** Pour un µ constant, il faut l'exponentielle.
- **Axe X = le temps**, toujours. Un graphe « F vs µ » est un outil de
  comparaison de stratégies, pas un run.

---

## 7. `Start` en rpm plutôt qu'en ml/min

Il faut une **calibration pompe** : `k` = ml/min par tr/min, obtenue en pesant
la sortie à un rpm connu (méthode gravimétrique). Alors :

```
Start_rpm = F₀[ml/min] / k
```

Fermentool n'a pas encore d'écran de calibration → pour l'instant, reste en
**ml/min**, ou fais la division à la main avec ton `k` mesuré.

Rappel : Fermentool ne touche **jamais** aux registres tête/tubing de la pompe.
En mode ml/min, c'est la pompe qui convertit ml/min → tr/min avec **sa** config
(à régler / calibrer sur la pompe elle-même).
