# Stijl

Ein Vektorzeichenprogramm für Linux mit GTK 4 und libadwaita.

Der Name kommt von **De Stijl** („Der Stil“), der niederländischen Kunstbewegung, der auch
Piet Mondrian angehörte. Bis Version 0.9.38 hieß das Programm „Mondrian“; Einstellungen,
Tastenkürzel und Bibliotheken von damals übernimmt Stijl beim ersten Start automatisch.

Version 0.9.41 – noch in der Testphase.

> **Hinweis:** Stijl wurde mit Hilfe von KI (Claude von Anthropic) geschrieben.
> Ideen, Anforderungen und Tests stammen vom Menschen, der Programmcode größtenteils von
> der KI.

## Funktionen

### Zeichnen
- **Werkzeuge (links):** Auswahl (V), Knoten bearbeiten (A), Formen (R: Rechteck, Ellipse, Vieleck, Stern), Formerstellung (Umschalt M oder K), Zeichenstift (P) / Kurvenzeichner (Umschalt P), Freihand (N), Radierer (Umschalt E) / Schere (C), Text (T) / Text auf Pfad (Umschalt T) / Textfeld, Hilfslinien (G). Die Ansicht verschiebt man mit Leertaste + Ziehen, mittlerer Maustaste oder dem Mausrad. Der Knopf oben links blendet die Namen der Werkzeuge ein oder aus
- **Werkzeuggruppen:** Knöpfe mit kleinem Dreieck haben Varianten – Rechtsklick öffnet die Auswahl
- **Zeichenstift/Kurvenzeichner:** mit Umschalt rastet die Richtung in 45°-Schritten und rechtwinklig zum vorigen Abschnitt ein; ein Punkt wird erst bei deutlichem Ziehen rund. Beim Kurvenzeichner zeigt kurzes Verweilen über dem Startpunkt die geschlossene Form als Vorschau; ein Klick schließt sie. Überall, wo kurzes Halten eine Vorschau zeigt (Kurvenzeichner, Bild oder Text in eine Form, Bibliothek auf ein Objekt), läuft dabei derselbe sich füllende Ring
- **Freihand:** einstellbare Glättung; liegen Anfang und Ende nah beieinander, wird die Form geschlossen
- **Knoten bearbeiten:** Knoten anklicken und ziehen, mehrere mit Umschalt oder Rahmen; am runden Ziehpunkt Ecken abrunden – auch bei Rechtecken und Ellipsen. Auch **mehrere Objekte** gleichzeitig: ein Rahmen aus dem Leeren erfasst die Knoten aller Objekte darin, Umschalt + Klick auf ein weiteres Objekt nimmt es mit allen Knoten dazu (noch einmal: wieder heraus). Verschieben, Runden, Ecke/Kurve und Löschen wirken auf alle gewählten Knoten zugleich
- **Vielecke und Sterne:** die Anzahl der Ecken lässt sich auch nachträglich in der Seitenleiste ändern (Lage, Größe, Drehung und Eckenradius bleiben)
- **Eckenradius:** an den runden Punkten im Knotenwerkzeug ziehen – einzelne Ecke oder mehrere gemeinsam; der Punkt bleibt immer im gleichen Abstand zum Rand der Rundung; begrenzt auf den Platz, den die angrenzenden Kanten lassen; der Radius wird beim Ziehen neben dem Zeiger angezeigt (beim Drehen ebenso der Winkel)
- **Radierer:** nimmt von gefüllten Formen Fläche weg und trennt Linien auf; Größe in der Seitenleiste oder mit [ und ]; mit Auswahl wirkt er nur auf die ausgewählten Objekte
- **Schere (C):** Werkzeugvariante neben dem Radierer (Rechtsklick). Eine Linie über Objekte ziehen zerschneidet sie: gefüllte Formen werden in Stücke geteilt (Kurven bleiben Kurven), Linien an den Schnittpunkten aufgetrennt; jedes Stück ist danach ein eigenes Objekt. Ein Schnitt, der nicht ganz durchgeht, ändert nichts. Mit Auswahl nur die ausgewählten Objekte
- **Formerstellung** (früher Formerstellung und Farbeimer): Alle Umrisse bilden Teilflächen – auch Zwischenräume zwischen Formen und Flächen, die nur von offenen Linien (z. B. Freihandstrichen) begrenzt werden; Kurven bleiben Kurven. Die Fläche unter dem Zeiger wird schon beim Zeigen markiert. Mit Auswahl zählen nur die ausgewählten Objekte (dann ist das ganze Netz der Teilflächen gestrichelt zu sehen), sonst alles Sichtbare.
  - **Klick** in eine Fläche: sie wird eine neue Form in der aktuellen Füllfarbe (die vorhandenen bleiben unverändert)
  - **Ziehen** über mehrere Flächen: sie werden zu einer Form vereinigt (Farbe der Fläche, an der man beginnt); die beteiligten Formen zerfallen in ihre übrigen Teilflächen, offene Linien bleiben erhalten
  - **Alt** beim Klicken oder Ziehen: die Flächen werden aus den Formen gelöscht
- **Pfadoperationen:** Vereinigen, Vorderes abziehen, Schnittmenge, Überlappung entfernen
- **Pfade verbinden** (Rechtsklick oder Strg J): offene Pfade werden an ihren nächstgelegenen Enden verbunden – kleine Lücken (unter 4 Bildschirmpunkten) verschmelzen zu einem Punkt, größere werden mit einem geraden Stück überbrückt. Ein einzelner offener Pfad wird geschlossen

### Text
- **Direkt im Dokument bearbeiten** (auch Text auf Pfad): Klick setzt den Cursor, Ziehen oder Doppelklick markiert, Esc beendet
- **Textoptionen:** eigener Ausklappbereich (zugeklappt stehen Schrift und Größe in der Kopfzeile); unten die Textstile der aktuellen Bibliothek zum Anklicken (wie die Bibliotheksfarben bei Füllung und Kontur); Schriftart aus einer Ausklappliste mit Suche, jede Schrift in ihrem eigenen Aussehen; darunter der **Schnitt** (alle Schnitte der Familie, z. B. Light, Regular, Medium, Semi Bold, Extra Bold, Black – jeweils auch kursiv), bei **variablen Schriften** zusätzlich ein Regler für die Strichstärke (100–900); unterstrichen, durchgestrichen, **Kapitälchen** und **Ligaturen** als Symbolknöpfe in derselben Reihe; „Text in Pfade umwandeln“ steht ebenfalls hier – jeder Buchstabe wird ein eigener Pfad, alle zusammen eine Gruppe (Strg Umschalt G löst sie, dann lassen sich einzelne Buchstaben verschieben)
- **Formatieren wie in einer Textverarbeitung:** In Textfeldern gelten Schrift, Größe und Auszeichnungen nur für den markierten Text; mit Cursor ohne Markierung für das Weiterschreiben; ist das Feld nur ausgewählt, für das ganze Feld. Sind im Feld verschiedene Schriften oder Größen, bleiben die Felder leer – eine Auswahl gilt dann für alles
- **Absatz:** links, rechts, zentriert, Blocksatz; Textfelder zusätzlich oben/mittig/unten; Zeilen- und Zeichenabstand. Enter beginnt einen neuen Absatz (mit einstellbarem Absatzabstand), Umschalt+Enter nur eine neue Zeile. Die Tab-Taste fügt einen Tabulator ein (Stopps im Abstand von 2,4 Schriftgrößen). Textfelder haben eine einstellbare **Spaltenzahl** (1–8): der Text läuft von Spalte zu Spalte, sobald die Höhe des Feldes erreicht ist. Ligaturen (fi, fl …) und Kapitälchen lassen sich je Text schalten
- **Aufzählungen und Einzug** (in Textfeldern, je Absatz): Knopf schaltet die Aufzählung ein und aus, der kleine Pfeil daneben wählt Punkte, Quadrate, Striche oder Nummerierung; Einzug vergrößern/verkleinern rückt Absätze stufenweise ein (eingerückte Nummern zählen eigenständig). Gilt beim Bearbeiten für die markierten Absätze, sonst für alle
- **Text auf Pfad:** über oder unter der Linie und in umgekehrter Richtung (z. B. unten am Kreis lesbar); auf geschlossenen Pfaden läuft der Text über den Anfang hinaus weiter
- **Textfeld:** wird beim Aufziehen mit markiertem Blindtext gefüllt – einfach lostippen
- **Text in Formen:** Textfeld auf eine Form ziehen und kurz halten – der Text bricht in der Form um
- **Hintergrund und Rahmen** für alle Textarten (Textfeld, Text, Text auf Pfad): Rahmenfüllung, Rahmenkontur und Abstand zum Rahmen

### Darstellung
- **Füllung und Kontur:** ausklappbar – zugeklappt stehen Deckkraft (bei der Kontur auch die Stärke) und ein Kreis mit der aktuellen Farbe; immer nur ein Bereich ist offen. Aufgeklappt: Reiter „Farbe“/„Verlauf“ und die **Pipette** daneben (Farbe aus dem Dokument oder einem Foto aufnehmen: ein Ring um den Zeiger zeigt die Farbe, Klick übernimmt sie, Esc bricht ab), Farbfeld, Farbton, Deckkraft (Leiste mit Zahlenfeld), Hex-Wert und die Bibliotheksfarben; „+“ nimmt die aktuelle Farbe in eine wählbare Bibliothek auf. Die Deckkraftleiste ändert bei mehreren Objekten nur die Deckkraft, jedes behält seine Farbe
- **Kontur:** Stärke, Lage (mittig, innen, außen), Ecken, Enden und Linienenden im aufgeklappten Bereich; „Kontur in Fläche umwandeln“ macht die Kontur zu einem eigenen Objekt (Ecken und Enden werden rund)
- **Bilder:** Deckkraft und **Temperatur** unter „Füllung“ (−100 % kalt, 0 % Original, 100 % warm), Rahmenlinie unter „Kontur“
- **Texte:** Rahmenfüllung und Rahmenkontur ausklappbar wie Füllung und Kontur; der Abstand zum Rahmen steht bei den Textoptionen
- **Kontur:** Strichmuster durchgezogen, gestrichelt, gepunktet, Strich-Punkt oder lange Striche (die Längen richten sich nach der Konturstärke; auch aus SVG- und Illustrator-Dateien übernommen); Ecken spitz, rund oder abgeschrägt; Enden gerade, rund oder überstehend; Linienenden (Pfeil, Punkt, Querstrich)
- **Farbverläufe:** für Füllung, Kontur, Schatten und Text, linear oder kreisförmig; Verlaufsleiste mit runden Farbstopps – Klick fügt einen hinzu, ziehen verschiebt, Entf entfernt; Winkel einstellbar; Verläufe lassen sich wie Farben in der Bibliothek speichern; die Verlaufslinie lässt sich im Dokument ziehen
- **Muster** (nicht für Text – dort unleserlich; Reiter neben „Farbe“ und „Verlauf“ bei der Füllung): Schachbrett, Punkte oder Streifen als Vorschaubilder, zwei frei wählbare Farben, Größe und Winkel; in SVG als echtes Muster gespeichert
- **Effekte:** Knopf „Effekte“ in der Schwebeleiste – wirkt auf alle ausgewählten Objekte gleichzeitig; Untermenüs öffnen sich daneben. Aus jeder Kategorie ist ein Effekt möglich, die Kategorien lassen sich kombinieren, die Stärke ist einstellbar (außer bei der Farbüberlagerung):
  - **Farbüberlagerung:** Aufhellen oder Abdunkeln (Standard ist Überdecken). Aufhellen mischt wie Lichtspots: ein einzelnes Feld zeigt seine Farbe, direkt aufeinanderfolgende Aufhellen-Objekte addieren ihr Licht, gewichtet mit der Deckkraft (Rot + Grün = Gelb, Rot + Grün + Blau = Weiß). Abdunkeln mischt wie übereinandergelegtes farbiges Glas Richtung Schwarz
  - **Textur:** **Milchglas** – was darunter liegt, erscheint weichgezeichnet (die Skala ist seit 0.9.19 verdoppelt: 100 % wirken wie früher 50 %); **Gebürstetes Metall** – feine waagerechte Striche mit metallischem Schimmer; **Hochglanz** – weiches Glanzlicht im oberen Teil, heller Rand und leichter Widerschein unten
  - **Kunsteffekte:** Körnung, Pixel, Wasserfarben – auf die Füllung von Formen und Text und ebenso auf Bilder. Sie verändern nur die Farben, nie die Deckkraft
  - **3D-Effekte:** geben dem Objekt ein Relief mit Licht (von links oben) und Schatten; bei Transparenz wird das Darunterliegende wie durch Glas verformt. Die **Tiefe** läuft von −100 % (eingedrückt) über 0 % (Ausgangszustand) bis +100 % (herausgedrückt); die **Beleuchtung** (0–100 %, Grundeinstellung 50 %) bestimmt, wie hell das Licht und wie dunkel der Schatten ausfallen. Formen: **Regentropfen** (abgeflachte Kugel), **Kugel** (komplett rund), **Kristall** (spitz zulaufend mit flacher Oberfläche), **Pyramide** (komplett spitz). Mit Textur kombinierbar; bei Bildern entfallen die 3D-Effekte. Ältere Dateien mit Konvex/Konkav werden als Regentropfen übernommen
  - **Bildfilter** (nur bei Bildern): Sepia, Schwarz-Weiß, Farben invertieren (Warm/Kalt ist jetzt der Regler „Temperatur“ bei der Füllung; ältere Dateien werden übernommen)
  - **Extras:** Schatten oder Spiegelung unter dem Objekt
  Jeder angewendete Effekt erscheint in den Eigenschaften als eigener Ausklappbereich mit Art und Stärke in der Kopfzeile und lässt sich dort entfernen. Die Art wählt man in einer Auswahlliste; alle Regler sehen gleich aus (runde Leiste wie bei der Deckkraft der Farbe) und haben daneben ein Feld zum Eintippen der Zahl
- **Bild nachzeichnen:** zeigt ein Bild flache Farbflächen (Logo, Illustration), erscheint in der Schwebeleiste „Nachzeichnen“. Die Zahl der Farben ist einstellbar; das Bild wird durch gruppierte, bearbeitbare Vektorflächen ersetzt (rückgängig machbar). Bei Fotos wird die Funktion nicht angeboten
- **Bilder:** einfügen (Strg I) oder auf die Zeichenfläche ziehen; auf eine Form ziehen und kurz halten setzt sie in die Form

### Anordnen
- **Schwebende Leiste:** über (oder bei wenig Platz unter) der Auswahl – Sperren/Entsperren, Duplizieren, zur Bibliothek hinzufügen (fragt: Farbe oder Stil), Effekte, Löschen, „…“ für weitere Möglichkeiten (dasselbe Menü wie beim Rechtsklick, Untermenüs öffnen sich daneben, nichts muss gescrollt werden); bei mehreren Objekten „Gruppieren“, bei Gruppen „Gruppierung auflösen"; bei genau zwei Objekten „Übergang“ (Zwischenformen mit einstellbarer Schrittzahl, Form, Farbe, Konturstärke und Deckkraft werden überblendet; beide Formen und die Zwischenformen werden gruppiert); ab drei Objekten oder Gruppen „Verteilen“ (horizontal oder vertikal, die äußeren bleiben stehen, alle Lücken werden gleich groß)
- **Rechtsklick:** nur, was sonst nirgends steht – Ausschneiden, Kopieren, Einfügen, Skalieren, Anordnen, Auf Ebene verschieben, Pfadoperationen
- **Gruppen:** Strg G fasst die Auswahl zusammen, Strg Umschalt G löst sie auf; ein Klick wählt immer die ganze Gruppe; Gruppen werden als `<g>` gespeichert, Gruppen aus anderen SVG-Dateien bleiben erhalten
- **Gedrehte Objekte:** der Auswahlrahmen dreht sich mit, Breite und Höhe lassen sich in Objektrichtung ändern
- **Sperren:** gesperrte Objekte lassen sich auswählen und umfärben, aber nicht verschieben, drehen, skalieren oder löschen
- **Intelligente Hilfslinien:** Einrasten an Ecken, Mitten, Ankerpunkten und Pfaden anderer Objekte, an den Zeichenflächen, an Hilfslinien und (optional) am Raster
- **Gleiche Abstände:** Beim Verschieben rastet die Auswahl ein, wenn die Lücke zu einem Nachbarn so groß ist wie eine andere Lücke in derselben Reihe (eine gleichmäßige Reihe lässt sich so fortsetzen) oder wenn links und rechts gleich viel Platz bleibt; in den Einstellungen abschaltbar
- **Hilfslinien:** klicken und ziehen – die Zugrichtung legt den Winkel fest (Umschalt: 45°-Schritte); auch aus den Linealen ziehbar. Beim Anlegen und beim späteren Verschieben rasten sie an Ankerpunkten, Ecken, Mitten und Kanten ein
- **Ausrichten:** links, mittig, rechts, oben, mittig, unten. Ein Objekt richtet sich an der Zeichenfläche aus, auf der es liegt; mehrere Objekte (Gruppen zählen als eines) wahlweise aneinander – am gemeinsamen Rahmen – oder als Ganzes an der Seite (Auswahlliste neben „Ausrichten“)
- **Skalieren …** (Rechtsklick): genau in Prozent, gleichmäßig oder Breite/Höhe getrennt, wahlweise mit Konturen und Schatten
- **Kopieren zwischen Dokumenten:** Strg C in einem Tab, Strg V in einem anderen – an derselben Stelle
- **Als Bild kopieren** (Rechtsklick oder Strg Alt C): die Auswahl – ohne Auswahl die Zeichenfläche – als Bild in die Zwischenablage, zum Einfügen in Chat, Mail oder Textverarbeitung

### Zeichenflächen und Ebenen
- **Leiste „Ebenen und Zeichenflächen“:** oben die Ebenen, unter einer Trennlinie die Zeichenflächen, jeweils mit eigenem „+“-Knopf
- **Mehrere Zeichenflächen** in einer Datei: jede hat eine eigene Zeile mit Vorschau, Zahnrad (Größe, Name) und Löschen. Solange die Leiste offen ist, sind die Zeichenflächen bearbeitbar: am Namen ziehen verschiebt sie (alles, was ganz darauf liegt, wandert mit), an den Anfassern ändert sich die Größe
- **Ebenen:** großes Vorschaubild nur mit den Objekten der Ebene, links ein Strich in der Ebenenfarbe, rechts Sperren, Ein-/Ausblenden und Löschen; per Ziehen umsortieren (ein Strich zeigt die Einfügestelle, die anderen Zeilen rücken schon auseinander); Doppelklick aufs Vorschaubild benennt um. Liegt die Auswahl auf einer Ebene, steht unten rechts im Vorschaubild ein kleines Kästchen in der Ebenenfarbe – es auf eine andere Ebene ziehen verschiebt die Auswahl dorthin (die Ebenen bleiben dabei an ihrem Platz, die Zielebene zeigt ein leeres Kästchen)
- **Hilfslinien liegen auf Ebenen:** neue Hilfslinien kommen auf die aktuelle Ebene; mit ihr ausgeblendet, gesperrt oder gelöscht
- **Exportieren:** PNG, JPG, PDF, SVG; **„Auswahl exportieren …“** (Menü) bzw. „Nur die Auswahl“ im Dialog exportiert nur die ausgewählten Objekte, so groß wie die Auswahl; bei mehreren Zeichenflächen wählt man sie per Vorschaubild aus (Alle/Keine) – PDF als eine Seite je Zeichenfläche, sonst eine Datei je Zeichenfläche. Bei PDF und SVG wählt man die **Qualität der Effekte** (klein 150 dpi, mittel 300 dpi, hoch 600 dpi)
  - **PDF:** Text durchsuchbar (Schriften werden eingebettet) oder in Umrisse umgewandelt; Farben **RGB**, **CMYK** oder **PDF/X-4** für Druckereien (mit eingebettetem Farbprofil der Druckbedingung, Endformat- und Anschnittrahmen); einstellbare **Beschnittzugabe** (bei PDF/X 3 mm vorbelegt). Stijl sucht Druckprofile in den üblichen Ordnern (z. B. „Coated FOGRA39“ von colord); andere wie „PSO Coated v3“ gibt es kostenlos bei der ECI (eci.org) und lassen sich im Dialog wählen. Neutrale Grautöne und Schwarz werden nur mit Schwarz (K) gedruckt. Text auf Pfad bleibt als Umriss
  - **SVG:** für andere Programme – Objekte mit Stijl-Effekten (Glas, 3D, Texturen, Kunsteffekte, Spiegelung, Bildfilter) werden als Bild eingebettet, Textfelder als fest gesetzte Zeilen geschrieben (Umbruch, Spalten, Blocksatz und Text in Formen sehen überall gleich aus); die Schriften lassen sich einbetten

### Bibliothek
- Knopf unten in der Werkzeugleiste; eigene Bibliotheken mit Farben, Stilen und Textstilen
- Mitgeliefert: die Standardbibliothek „Stijl“ mit Grundfarben und den Textstilen Überschrift, Text und Zitat – sie lässt sich nicht ändern; Hinzugefügtes landet automatisch in einer eigenen Bibliothek
- Über das „+“ am Ende jedes Abschnitts neu anlegen oder aus der Auswahl übernehmen, auch per Rechtsklick auf ein Objekt; neue Farben und Stile wählt man mit demselben Farbwähler wie in den Eigenschaften
- Ist bei Füllung oder Kontur der Reiter „Verlauf“ offen, färbt ein Klick auf eine Bibliotheksfarbe nur den gewählten Farbstopp
- Farben und Stile lassen sich per Rechtsklick benennen (freiwillig); der Name steht unter dem Feld
- Die Felder brechen je nach Breite der Leiste um
- **Grafiken:** ausgewählte Objekte (z. B. ein Logo) über „+“ unter „Grafiken“, den Rechtsklick oder die schwebende Leiste als Grafik speichern; per Klick oder Ziehen in jedes Dokument kopieren – mehrere Objekte kommen als Gruppe
- Stile werden wie Farben als Kreis gezeigt
- Klick überträgt Farbe bzw. Stil auf die Auswahl; oder direkt auf ein Objekt ziehen – nach kurzer Wartezeit erscheint eine Vorschau
- Importieren, Exportieren, Ordner öffnen, Neu einlesen über das Menü (⋮)

### Ansicht „Notizheft“
- Statt der Zeichenfläche (alle Zeichenflächen nebeneinander) lassen sich die Seiten wie in einem Heft zeigen: immer nur eine Seite, was außerhalb liegt, ist ausgeblendet und nicht anklickbar
- Scrollen bewegt die Seite; wer am Seitenende weiterscrollt, blättert zur nächsten Seite (am Anfang zur vorigen). Bild ↓ / Bild ↑ blättern ebenso, und in der Leiste „Zeichenflächen“ springt ein Klick auf das Vorschaubild zur Seite
- Unten in der Statusleiste steht „Seite 2 von 5“ mit Pfeilen zum Blättern; der Heft-Knopf daneben (oder Strg Umschalt N) wechselt jederzeit zwischen Zeichenfläche und Notizheft, ebenso Einstellungen › Ansicht. Die Wahl wird gespeichert und gilt für alle Dokumente

### Oberfläche
- **Sprache:** Deutsch oder britisches Englisch. Stijl richtet sich nach der Sprache des Systems (deutschsprachig → Deutsch, sonst Englisch); in den Einstellungen unter „Sprache“ lässt sich das festlegen (wirkt nach einem Neustart). Die Übersetzung steht in `po/en_GB.po` und lässt sich z. B. mit Poedit bearbeiten; `po/stijl.pot` ist die Vorlage für weitere Sprachen
- **Fehler melden:** Hauptmenü › „Fehler melden …“ öffnet im Browser ein neues Issue mit Vorlage und Angaben zu Version und System
- **Updates (Windows und macOS):** Stijl fragt einmal am Tag bei GitHub nach, ob es eine neue Version gibt, und zeigt dann einen Hinweis mit „Herunterladen“. Dabei wird nur die Versionsnummer abgefragt; GitHub sieht wie bei jedem Seitenaufruf die IP-Adresse. Abschalten oder sofort suchen: Einstellungen › Ansicht › Updates. Unter Linux ist die Suche standardmäßig aus, als Flatpak oder Snap gibt es sie nicht (dort aktualisiert die Paketverwaltung)
- **Absturzbericht:** Stürzt Stijl ab, wird ein Bericht gespeichert (Version, System, Fehlerstelle – keine Zeichnungen). Beim nächsten Start fragt Stijl, ob er gesendet werden soll; man kann ihn vorher ansehen, kopieren oder verwerfen
- Startseite (Strg N): ohne Symbol und Begrüßung; oben die zuletzt geöffneten Dokumente in einer Reihe, am Ende die gestrichelte Kachel „Öffnen …“; darunter die Formate in den Reitern „Print“ und „Online“ mit Suchfeld daneben, am Ende des Rasters die gestrichelte Kachel „Eigenes Format …“
- Tabs für mehrere Dokumente, Statusleiste mit Speicherstand, Format und Zoom. Dateien, die vom Desktop aus (Doppelklick, „Öffnen mit“) geöffnet werden, kommen als neuer Tab in das schon offene Fenster
- Ohne Auswahl steht in den Eigenschaften „Nichts ausgewählt“
- Die Position (Drehen, Spiegeln, an der Seite ausrichten) steht fest unten in den Eigenschaften
- Links unten in der Werkzeugleiste öffnen Eigenschaften, Ebenen und Bibliothek je eine ausklappbare Leiste – immer nur eine zugleich; sie lässt sich mit der Maus verbreitern, das Mausrad rollt immer die Leiste
- Einstellungen (Strg ,): graue Seitenleiste mit den Kategorien **Ansicht** (Farbschema System/Hell/Dunkel, Hintergrund Standard/Dunkel/Schachbrett, Ansicht Zeichenfläche/Notizheft, Lineale, Maßeinheit – metrisch, imperial oder Pixel), **Ausrichten** („Einrasten an“: Hilfslinien, Ecken, Mitten, Ankerpunkte, Seite, Pfade, Abstände und Raster einzeln oder mit „Alle an / Alle aus“; dazu die Wartezeit beim Einsetzen in Formen) und **Shortcuts**; die Lupe oben durchsucht alle Einstellungen. Bei den Shortcuts sind alle Kürzel aufgelistet, jedes lässt sich ändern (mit Konfliktprüfung) und einzeln oder insgesamt auf den Standard zurücksetzen; gespeichert in `~/.config/stijl/shortcuts.ini`
- Die Beschriftungen Eigenschaften, Ebenen und Bibliothek sind einheitlich groß
- Dateien werden als SVG gespeichert; Ebenen, Schatten (als Filter) und Verläufe sind auch für andere Programme lesbar. Gespeichert wird sicher: erst in eine temporäre Datei, dann wird sie umbenannt – bricht das Speichern ab, bleibt die alte Datei heil
- **Wiederherstellung nach einem Absturz:** ungespeicherte Änderungen werden alle 20 Sekunden (und bei einem Programmfehler sofort) in eine eigene Wiederherstellungsdatei gesichert – die Originaldatei wird dabei nie verändert. Nach einem Absturz bietet Stijl beim nächsten Start an, die Dokumente wiederherzustellen
- **SVG-Dateien anderer Programme:** CSS-Klassen (z. B. aus Illustrator), verlinkte Bilder neben der Datei und Dateien in ISO-8859-1 werden gelesen. Enthält die Datei Inhalte, die Stijl nicht bearbeiten kann (Schnittpfade, Masken, Filter, `<use>`, Muster, verschachtelte Gruppen …), erscheint ein Hinweis mit der Liste, und die Datei wird wie ein Import behandelt: Speichern legt eine neue Datei an, das Original bleibt unverändert
- **Effekte schnell:** Schatten, Wasserfarben, Glas-, 3D- und Texturbilder werden zwischengespeichert und beim Verschieben, Skalieren, Zoomen und Verschieben der Ansicht nicht neu berechnet – erst danach. Auf HiDPI-Bildschirmen in voller Auflösung
- **Illustrator-Dateien (.ai) und PDF öffnen:** über Öffnen, Ziehen ins Fenster oder „Öffnen mit“. Die Datei wird eingelesen und wie eine SVG-Datei behandelt; sie erscheint als unbenannter Tab, Speichern fragt nach einem SVG-Namen – die .ai-Datei wird nie verändert. Voraussetzung: in Illustrator mit der Option „PDF-kompatible Datei erstellen“ gespeichert (das ist die Voreinstellung). Übernommen werden Ebenen (auch ausgeblendete), Pfade mit Füllung und Kontur, Verläufe (linear und radial), Beschneidungsmasken, Gruppen mit Deckkraft und Mischmodus, Text (als Text, mit ähnlicher installierter Schrift), Bilder und alle Farbräume (CMYK wird nach RGB umgerechnet); mehrere Seiten werden zu Zeichenflächen. Nicht übernommen: Deckkraftmasken, Muster- und Netzverläufe (vereinfacht), CMYK-JPEG- und JPEG-2000-Bilder, verschlüsselte Dateien

## Bauen

Benötigt GTK ≥ 4.12, libadwaita ≥ 1.5 und eine aktuelle Rust-Version (z. B. über [rustup](https://rustup.rs)).

```bash
# Ubuntu 24.04+ / Debian 13
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev
# Fedora 40+
sudo dnf install gcc gtk4-devel libadwaita-devel
# Arch
sudo pacman -S base-devel gtk4 libadwaita

cd Stijl
cargo run --release
```

### Installieren

```sh
./install.sh
```

Baut Stijl und installiert es für den aktuellen Benutzer (ohne Root): Programm nach
`~/.local/bin/stijl`, Starter in die App-Übersicht. Liegt ein App-Symbol als
`data/io.github.benhuesing.Stijl.svg` vor, wird es mit installiert. Nach einem Update
erneut ausführen. Entfernen mit `./uninstall.sh`.

### Windows und macOS

Stijl bleibt ein GNOME-Programm, läuft aber auch unter Windows 10/11 (64 Bit) und macOS
15 oder neuer auf Apple-Silicon-Macs (M1 und neuer) – mit GNOME-Aussehen.

**Mit GitHub (empfohlen):** Unter „Actions“ › „Release“ › „Run workflow“ starten. Nach
etwa 30 Minuten liegen `Stijl-<Version>-Setup.exe` und `Stijl-<Version>-macOS.dmg` als
Download bei der Aktion (unten unter „Artifacts“).

**Neue Version veröffentlichen:**
1. Version in `Cargo.toml` erhöhen und einen Eintrag in `data/io.github.benhuesing.Stijl.metainfo.xml` (`<releases>`) ergänzen
2. Hochladen und ein Tag setzen, z. B. `git tag v0.9.40 && git push --tags` (oder auf GitHub unter „Releases“ › „Draft a new release“ ein neues Tag anlegen)
3. Die Aktion legt einen **Release-Entwurf** mit Installer und Disk-Image an. Text prüfen und auf „Publish release“ klicken – erst dann sehen die Programme die neue Version

**Auf dem eigenen Rechner bauen:**
- Windows: [MSYS2](https://www.msys2.org) und [Inno Setup 6](https://jrsoftware.org/isdl.php) installieren, im Startmenü „MSYS2 UCRT64“ öffnen, in den Stijl-Ordner wechseln (z. B. `cd /c/Users/Name/Downloads/Stijl`) und `bash packaging/windows/build.sh` ausführen. Ergebnis: `target/windows/Stijl-<Version>-Setup.exe` und der Programmordner `target/windows/Stijl`, der auch ohne Installation läuft (`bin\stijl.exe`)
- macOS: [Homebrew](https://brew.sh) und [Rust](https://rustup.rs) installieren, dann im Stijl-Ordner `bash packaging/macos/build.sh`. Ergebnis: `target/macos/Stijl.app` und `target/macos/Stijl-<Version>-macOS.dmg`

Der Windows-Installer legt Stijl ins Startmenü (wahlweise auf den Desktop), trägt es bei
„Öffnen mit“ für SVG, AI und PDF ein (die Standard-App bleibt unverändert) und lässt sich in
den Windows-Einstellungen wieder entfernen. Auf dem Mac zieht man Stijl aus dem
Disk-Image in „Programme“.

**Warnungen beim ersten Start:** Die Programme sind nicht bei Microsoft bzw. Apple
signiert. Windows zeigt „Der Computer wurde durch Windows geschützt“ – über „Weitere
Informationen“ › „Trotzdem ausführen“ geht es weiter. macOS meldet „Stijl“ wurde nicht
geöffnet“ – dann unter Systemeinstellungen › Datenschutz & Sicherheit ganz unten „Dennoch
öffnen“ wählen. Beides ist nur beim ersten Start nötig.

Unterschiede zu Linux: Beim SVG-Export lassen sich keine Schriften einbetten, und der Regler
für variable Schriften fehlt (beides braucht Fontconfig). Auf dem Mac gilt ⌘ statt Strg.
Einstellungen und Wiederherstellungsdateien liegen unter Windows in
`%LOCALAPPDATA%\stijl`, auf dem Mac in `~/.config/stijl` und `~/.local/share/stijl`. Weitere Hinweise:
[PORTING.md](PORTING.md).

### Wo liegt was?

| Was | Ort |
|---|---|
| Bibliotheken | `Dokumente/Stijl/Bibliotheken/` – je Bibliothek eine SVG-Datei; zum Teilen kopieren oder den Ordner mit einem Cloud- oder Sync-Dienst abgleichen |
| Einstellungen | `~/.config/stijl/settings.ini` |
| Zuletzt geöffnet | `~/.config/stijl/recent` |
| Wiederherstellung | `~/.local/state/stijl/recovery/` |
| Tastenkürzel | `~/.config/stijl/shortcuts.ini` |

## Tastenkürzel

Standardbelegung; in den Einstellungen unter „Shortcuts“ änderbar.

| Kürzel | Funktion |
|---|---|
| V, A, R, P, N, T, G, H, K | Werkzeug wechseln |
| Umschalt P / T / E / M | Kurvenzeichner / Text auf Pfad / Radierer / Formerstellung |
| Leertaste halten / mittlere Maustaste | Ansicht verschieben |
| Strg + Mausrad, Zwei-Finger-Zoom | Zoomen |
| Strg + / Strg − / Strg 0 / Strg 1 | Vergrößern / Verkleinern / Alle Zeichenflächen (im Notizheft: ganze Seite) / 100 % |
| Strg Umschalt N | Notizheft-Ansicht ein/aus |
| Bild ↓ / Bild ↑ | Nächste / vorige Seite (Notizheft) |
| Umschalt beim Ziehen | Proportional, gerade Linien, 45°-Schritte |
| Alt beim Ziehen | Verschieben: Kopie ziehen (Original bleibt); Formen: vom Mittelpunkt aus; Formerstellung: Flächen löschen |
| Enter / Doppelklick | Zeichenstift-Pfad beenden |
| Entf | Löschen |
| Pfeiltasten (+ Umschalt) | 1 px (10 px) verschieben |
| Strg C / X / V / D / A | Kopieren / Ausschneiden / Einfügen / Duplizieren / Alles auswählen |
| Strg Z / Strg Umschalt Z | Rückgängig / Wiederholen |
| Strg G / Strg Umschalt G | Gruppieren / Gruppierung auflösen |
| Strg ] / Strg [ (+ Umschalt) | Nach vorne / hinten (ganz) |
| Umschalt H / Umschalt V | Horizontal / vertikal spiegeln |
| C | Schere |
| Strg J | Pfade verbinden |
| Strg Alt C | Als Bild kopieren |
| [ / ] | Radierer kleiner / größer |
| Strg W | Tab schließen |
| Strg I | Bild einfügen |
| Strg , | Einstellungen |
| F8 / F9 | Werkzeugleiste / Eigenschaften |
| Strg N / O / S / Umschalt S / E | Neu / Öffnen / Speichern / Speichern unter / Exportieren |

## Aufbau des Codes

| Datei | Inhalt |
|---|---|
| `geom.rs` | Punkte, Rechtecke, Matrizen, Bézier-Hilfen |
| `model.rs` | Dokument, Zeichenflächen, Ebenen, Objekte, Formen, Stile |
| `editor.rs` | Werkzeuge, Auswahl, Rückgängig (ohne GTK, getestet) |
| `boolean.rs` | Pfadoperationen (getestet) |
| `shapeops.rs` | Formerstellung, Radierer, Schere (getestet) |
| `library.rs` | Bibliotheken (getestet) |
| `fsutil.rs` | Sicheres Speichern, Wiederherstellungsdateien (getestet) |
| `icc.rs`, `pdfx.rs` | Farbprofile (sRGB → CMYK) und PDF-Umwandlung nach CMYK/PDF/X-4 mit eigenem Deflate (getestet) |
| `keys.rs` | Tastenkürzel, eigene Belegung, Konfliktprüfung (getestet) |
| `xml.rs`, `svg.rs` | SVG lesen und schreiben (getestet) |
| `imgfx.rs`, `trace.rs` | Bildfilter (Sepia, Temperatur, Kunsteffekte), Rechenteile der 3D-Effekte und Texturen sowie Bildnachzeichner (getestet) |
| `pdf.rs`, `ai.rs` | PDF-Leser (Inflate, Funktionen, Farbräume) und Import von Illustrator-Dateien als SVG (getestet) |
| `render.rs` | Zeichnen mit Cairo/Pango, Effekte |
| `export.rs` | PNG, JPG, PDF, SVG |
| `window.rs` | Oberfläche (libadwaita) |
| `i18n.rs`, `po/` | Übersetzungen (Deutsch → britisches Englisch, getestet) |
| `icons.rs` | eigene Symbole |
| `update.rs` | Suche nach neuen Versionen (getestet) |
| `packaging/`, `.github/` | Windows-Installer, macOS-Bundle, automatische Builds |

Tests: `cargo test`. Auf GitHub baut die Aktion „Prüfen“ bei jedem Hochladen unter Linux
und führt die Tests aus; ein rotes Kreuz unter „Actions“ zeigt Fehler.

Eigene Angaben in SVG-Dateien (Effekte, Textfelder, Ebenen …) stehen im Namensraum
`mondrian:` – der alte Name bleibt dort, damit ältere Dateien lesbar bleiben.

Hinweise zu Windows und macOS: [PORTING.md](PORTING.md)

## Lizenz

Copyright © 2026 Bendix Hüsing

Stijl ist freie Software unter der GNU General Public License, Version 3 oder
(nach Wahl) jeder späteren Version – siehe [LICENSE](LICENSE). Sie wird ohne jede
Gewährleistung bereitgestellt, soweit das Gesetz es zulässt.

Die Windows-Ausgabe enthält Bibliotheken anderer Projekte (u. a. GTK, libadwaita, Cairo,
Pango, GLib) unter ihren eigenen Lizenzen; die Liste steht in `THIRD-PARTY.txt` im
Programmordner.
