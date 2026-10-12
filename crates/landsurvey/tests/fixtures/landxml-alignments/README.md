# LandXML alignment fixtures

Real alignment exports from several road-design packages, used by
`tests/landxml_alignment_samples.rs` to check spiral (clothoid) geometry,
stationing and vertical profiles against each vendor's own numbers. The files
are unmodified copies.

| File | Producer | Source | Licence |
|---|---|---|---|
| `aplitop-1.xml` | Aplitop MDT 8.0 | buildingSMART [IFC-infra-unit-test](https://github.com/bSI-InfraRoom/IFC-infra-unit-test) `Alignment-Aplitop-1/UT-Alignment-Aplitop-1.xml` | CC BY 4.0, (C) original authors |
| `aplitop-2.xml` | Aplitop MDT 8.0 | same repo, `Alignment-Aplitop-2/Alignment-Aplitop-2.XML` | CC BY 4.0, (C) original authors |
| `indot-openroads.xml` | Bentley OpenRoads Designer 10.09 (INDOT) | same repo, `Alignment-INDOT/PR_Twin_Branch_section_alignment.xml` | CC BY 4.0, (C) original authors |
| `12d-example.xml` | 12d Model 15 | [RustedGeom/rusted-geom](https://github.com/RustedGeom/rusted-geom) `showcase/public/landxml/12DExample.xml` | MIT, (c) 2024–2026 RustedGeom Contributors |
| `openroads-spiral-double-radius.xml` | Bentley OpenRoads Designer 10.12 | same repo, `showcase/public/landxml/OpenRoadSpiralDoubleRadius.xml` | MIT, (c) 2024–2026 RustedGeom Contributors |

The buildingSMART files are licensed under the Creative Commons Attribution
4.0 International License (<http://creativecommons.org/licenses/by/4.0/>).
The rusted-geom files are under the MIT License; its copyright notice is
reproduced above.
