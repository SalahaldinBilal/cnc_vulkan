// Writes the function matches of a Version Tracking session as "<source entry> <destination entry>
// <status> <score> <source size> <destination size>" lines, one per function match.
// Args: <output file> <session path in the project, e.g. /zh_to_generals>
// @category CnC
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;

import ghidra.app.script.GhidraScript;
import ghidra.feature.vt.api.main.VTAssociation;
import ghidra.feature.vt.api.main.VTAssociationType;
import ghidra.feature.vt.api.main.VTMatch;
import ghidra.feature.vt.api.main.VTMatchSet;
import ghidra.feature.vt.api.main.VTSession;
import ghidra.framework.model.DomainFile;

public class ExportVT extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        DomainFile file = state.getProject().getProjectData().getFile(args[1]);
        VTSession session = (VTSession) file.getDomainObject(this, false, false, monitor);
        try (PrintWriter out = new PrintWriter(args[0], StandardCharsets.UTF_8)) {
            for (VTMatchSet set : session.getMatchSets()) {
                String correlator = set.getProgramCorrelatorInfo().getName();
                for (VTMatch match : set.getMatches()) {
                    VTAssociation a = match.getAssociation();
                    if (a.getType() != VTAssociationType.FUNCTION) {
                        continue;
                    }
                    out.println(a.getSourceAddress() + " " + a.getDestinationAddress() + " " + a.getStatus() + " "
                        + match.getSimilarityScore().getScore() + " " + match.getSourceLength() + " "
                        + match.getDestinationLength() + " " + correlator.replace(' ', '_'));
                }
            }
        } finally {
            session.release(this);
        }
    }
}
